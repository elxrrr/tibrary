#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Write},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
};
use tauri::{Emitter, Manager};
use tokio::sync::oneshot;

struct Backend {
    input: Mutex<Option<ChildStdin>>,
    child: Mutex<Option<Child>>,
    pending: Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>,
    serial: AtomicU64,
    closing: AtomicBool,
    finished: AtomicBool,
}
impl Backend {
    async fn call(&self, method: String, args: Value) -> Result<Value, String> {
        let id = self.serial.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        let data = json!({"id":id,"method":method,"args":args}).to_string() + "\n";
        let written = match self.input.lock().unwrap().as_mut() {
            Some(input) => input.write_all(data.as_bytes()).map_err(|e| e.to_string()),
            None => Err("The Python service is not running. Restart Tibrary.".into()),
        };
        if let Err(e) = written {
            self.pending.lock().unwrap().remove(&id);
            return Err(e);
        }
        match tokio::time::timeout(std::time::Duration::from_secs(120), rx).await {
            Ok(Ok(value)) => value,
            _ => {
                self.pending.lock().unwrap().remove(&id);
                Err(
                    "The service did not answer. Check Activity; cancellation remains available."
                        .into(),
                )
            }
        }
    }
}
#[tauri::command]
async fn backend_call(
    state: tauri::State<'_, Arc<Backend>>,
    method: String,
    args: Value,
) -> Result<Value, String> {
    state.call(method, args).await
}
#[tauri::command]
fn open_external(url: String) -> Result<(), String> {
    let parsed = url::Url::parse(&url).map_err(|e| e.to_string())?;
    let host = parsed.host_str().unwrap_or("");
    if parsed.scheme() != "https" || !(host == "tidal.com" || host.ends_with(".tidal.com")) {
        return Err("Only verified service links can open from this action.".into());
    }
    Command::new("open")
        .arg(url)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}
#[tauri::command]
fn reveal_file(path: String) -> Result<(), String> {
    let p = std::path::PathBuf::from(path);
    if !p.is_absolute() || !p.exists() {
        return Err("The file is unavailable. Reconnect the library or update its index.".into());
    }
    Command::new("open")
        .arg("-R")
        .arg(p)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}
#[tauri::command]
fn save_export(path: String, content: String) -> Result<(), String> {
    let p = std::path::PathBuf::from(path);
    if !p.is_absolute()
        || !matches!(
            p.extension().and_then(|v| v.to_str()),
            Some("json" | "csv" | "txt" | "m3u")
        )
    {
        return Err("Choose a JSON, CSV or text export file.".into());
    }
    // Never replace a user's existing file without a separate review step.
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(p)
        .map_err(|e| e.to_string())?;
    file.write_all(content.as_bytes())
        .map_err(|e| e.to_string())
}
fn main() {
    let backend = Arc::new(Backend {
        input: Mutex::new(None),
        child: Mutex::new(None),
        pending: Mutex::new(HashMap::new()),
        serial: AtomicU64::new(1),
        closing: AtomicBool::new(false),
        finished: AtomicBool::new(false),
    });
    let service = backend.clone();
    tauri::Builder::default().plugin(tauri_plugin_dialog::init()).manage(backend)
      .invoke_handler(tauri::generate_handler![backend_call,open_external,reveal_file,save_export])
      .setup(move |app|{
        let mut command;
        if cfg!(debug_assertions){
            let root=std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
            command=Command::new(root.join(".venv/bin/python"));command.arg("-m").arg("library_manager.sidecar");command.env("PYTHONPATH",root.join("app"));
        }else{
            let exe=std::env::current_exe()?.parent().unwrap().join("tibrary-service");command=Command::new(exe);
        }
        if std::env::var_os("TIBRARY_DEMO").is_some(){command.arg("--demo");}
        if let Ok(db)=std::env::var("TIBRARY_TEST_DB"){command.arg("--db").arg(db);}
        let logs=app.path().app_log_dir()?;std::fs::create_dir_all(&logs)?;
        let logfile=logs.join("python-service.log");
        if std::fs::metadata(&logfile).map(|m|m.len()>2_000_000).unwrap_or(false){let _=std::fs::rename(&logfile,logs.join("python-service.previous.log"));}
        let errors=std::fs::OpenOptions::new().create(true).append(true).open(logfile)?;
        let mut child=command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::from(errors)).spawn()?;
        *service.input.lock().unwrap()=child.stdin.take();let stdout=child.stdout.take().unwrap();*service.child.lock().unwrap()=Some(child);
        let apphandle=app.handle().clone();let s=service.clone();
        std::thread::spawn(move||{
            for line in BufReader::new(stdout).lines(){
                let Ok(line)=line else{break};let Ok(value)=serde_json::from_str::<Value>(&line) else{continue};
                if let Some(id)=value.get("id").and_then(Value::as_u64){
                    if let Some(sender)=s.pending.lock().unwrap().remove(&id){let result=if let Some(error)=value.get("error"){Err(error.as_str().unwrap_or("Service error").into())}else{Ok(value["result"].clone())};let _=sender.send(result);}
                }else{let _=apphandle.emit("backend-event",value);}
            }
            *s.input.lock().unwrap()=None;
            for (_,sender) in s.pending.lock().unwrap().drain(){let _=sender.send(Err("The Python service stopped. Restart Tibrary; completed files remain indexed.".into()));}
            let _=apphandle.emit("backend-event",json!({"event":"stopped"}));
        });Ok(())
      })
      .on_window_event(|window,event|{
        if let tauri::WindowEvent::CloseRequested{api,..}=event{
            api.prevent_close();request_shutdown(window.app_handle().clone());
        }
      }).build(tauri::generate_context!()).expect("Could not start Tibrary")
      .run(|app,event|{
        if let tauri::RunEvent::ExitRequested{api,..}=event{
            if !app.state::<Arc<Backend>>().finished.load(Ordering::SeqCst){
                api.prevent_exit();request_shutdown(app.clone());
            }
        }
      });
}
fn request_shutdown(app: tauri::AppHandle) {
    let state = app.state::<Arc<Backend>>().inner().clone();
    if state.closing.swap(true, Ordering::SeqCst) {
        return;
    }
    let _ = app.emit("backend-event", json!({"event":"closing"}));
    tauri::async_runtime::spawn(async move {
        loop {
            match state.call("shutdown".into(), json!({})).await {
                Ok(v) if v["safe"].as_bool() == Some(true) => break,
                Err(_) => break,
                _ => {}
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
        state.input.lock().unwrap().take();
        let child = state.child.lock().unwrap().take();
        if let Some(mut child) = child {
            let _ = tauri::async_runtime::spawn_blocking(move || child.wait()).await;
        }
        state.finished.store(true, Ordering::SeqCst);
        app.exit(0);
    });
}
