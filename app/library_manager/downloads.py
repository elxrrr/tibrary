"""Run the isolated downloader and persist only explicitly completed queue items."""
import json
import os
from pathlib import Path
import selectors
import subprocess
import time
from .core import now
from .tidal import CatalogueError


def index_downloaded_files(store,files):
    """Immediately index downloads which land inside a registered library."""
    from .core import read_metadata
    roots=[Path(r['root']).resolve() for r in store.rows('SELECT root FROM roots')]
    indexed=0
    for value in files or []:
        path=Path(value)
        if path.suffix.lower() not in ('.flac','.m4a','.mp4','.alac','.mp3','.aif','.aiff','.wav','.wave') or not path.is_file():continue
        resolved=path.resolve()
        root=max((r for r in roots if resolved.is_relative_to(r)),key=lambda r:len(r.parts),default=None)
        if root is None:continue
        try:metadata=read_metadata(resolved);stat=resolved.stat()
        except (OSError,ValueError):continue
        with store.connect() as db:
            db.execute('INSERT OR REPLACE INTO local_files VALUES(?,?,?,?,?,?,1)',
                       (str(resolved),str(root),stat.st_size,stat.st_mtime_ns,json.dumps(metadata),None))
        indexed+=1
    return indexed


def download_approved(store, output, cancel, progress, authenticate, process_factory=subprocess.Popen, connect_only=False):
    rows=store.rows("SELECT * FROM queue WHERE approved=1 AND decision='queued' ORDER BY id")
    if connect_only:rows=[]
    if not rows and not connect_only:return 'No approved items to download'
    root=Path(__file__).resolve().parents[2]
    from .backends import active_runtime
    try:python=active_runtime()
    except ValueError as exc:raise CatalogueError(str(exc)) from None
    if not python.is_file():raise CatalogueError('Tidaler runtime missing. Run app/tools/Setup downloads.command.')
    from .tag_io import _lofty
    env=dict(os.environ,TIBRARY_NATIVE_MODULE=_lofty.__file__,PYTHONUNBUFFERED='1',XDG_CONFIG_HOME=str(store.path.parent/'downloader-session'))
    try:
        import imageio_ffmpeg
        env['TIBRARY_FFMPEG']=imageio_ffmpeg.get_ffmpeg_exe()
    except ImportError:pass
    settings=dict(store.preferences('downloads'),**store.preferences('provider'))
    request=dict(output=str(Path(output).expanduser().resolve()),settings=settings,layout=store.preferences('organisation'),items=[dict(id=r['id'],release=json.loads(r['payload'])) for r in rows])
    process=process_factory([str(python),'-u',str(Path(__file__).with_name('download_bridge.py'))],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,env=env,start_new_session=True)
    selector=selectors.DefaultSelector();selector.register(process.stdout,selectors.EVENT_READ)
    process.stdin.write((json.dumps(request)+'\n').encode());process.stdin.flush()
    completed=0;buffer=b'';cancelled_at=None;saw_finished=False
    by_id={str(r['id']):r for r in rows}
    try:
        while True:
            if cancel() and cancelled_at is None:
                process.terminate();cancelled_at=time.monotonic();progress('Stopping downloader · allowing the current request to finish')
            if cancelled_at and time.monotonic()-cancelled_at>25:
                process.kill()
            events=selector.select(.2)
            if events:
                chunk=os.read(process.stdout.fileno(),65536)
                if not chunk:break
                buffer+=chunk
                while b'\n' in buffer:
                    line,buffer=buffer.split(b'\n',1)
                    try:message=json.loads(line)
                    except (ValueError,UnicodeDecodeError):continue
                    event=message.get('event')
                    if event in ('log','error'):progress(message.get('message','Downloader update'))
                    elif event=='auth':
                        reply=authenticate(message['url'],cancel)
                        if not reply:
                            process.stdin.write(b'{"redirect":""}\n');process.stdin.flush()
                            process.terminate();cancelled_at=time.monotonic()
                        else:
                            process.stdin.write((json.dumps(dict(redirect=reply))+'\n').encode());process.stdin.flush()
                    elif event=='completed' and str(message.get('id')) in by_id:
                        row=by_id.pop(str(message['id']))
                        payload=json.loads(row['payload'])
                        payload['downloaded_files']=message.get('files',[])
                        with store.connect() as db:
                            db.execute("UPDATE queue SET payload=?,decision='downloaded',approved=0,updated=? WHERE id=? AND payload=? AND approved=1",(json.dumps(payload),now(),row['id'],row['payload']))
                        completed+=1;progress(f'Completed {completed}/{len(rows)} approved releases')
                        indexed=index_downloaded_files(store,message.get('files',[]))
                        if indexed:progress(f'Library updated · {indexed} downloaded tracks indexed')
                    elif event=='finished':saw_finished=True
            if process.poll() is not None and not events:break
        code=process.wait(timeout=5)
        if cancelled_at:return f'Download cancelled · {completed} completed; remaining items stay queued'
        if code or not saw_finished:raise CatalogueError(f'Download stopped · {completed} completed; remaining items stay queued. See activity for details.')
        if connect_only:return 'Download account connected'
        return f'Download finished · {completed} approved releases saved to {output}'
    finally:
        selector.close()
        if process.poll() is None:
            process.terminate()
            try:process.wait(timeout=5)
            except subprocess.TimeoutExpired:process.kill();process.wait(timeout=5)
        process.stdin.close();process.stdout.close()


def check_download_connection(store):
    root = Path(__file__).resolve().parents[2]
    from .backends import active_runtime
    try:python=active_runtime()
    except ValueError:return 'Download account: install the streaming components in Settings.'
    if not python.is_file(): return 'Download account: install the downloader using app/tools/Setup downloads.command.'
    config = store.path.parent / 'downloader-session'
    if not config.is_dir(): return 'Download account: not connected. Connect in Settings → Connections.'
    from .tag_io import _lofty
    env = dict(os.environ, TIBRARY_NATIVE_MODULE=_lofty.__file__, PYTHONUNBUFFERED='1', XDG_CONFIG_HOME=str(config))
    try:
        timeout=max(5,min(60,int(store.preferences('provider').get('request_timeout_sec',20))))
        result = subprocess.run([str(python), '-u', str(Path(__file__).with_name('download_bridge.py'))],
            input=json.dumps(dict(output=str(store.path.parent),items=[],check_only=True))+'\n',
            text=True, capture_output=True, env=env, timeout=timeout)
        for line in result.stdout.splitlines():
            try: message = json.loads(line)
            except ValueError: continue
            if message.get('event') == 'connection':
                return 'Download account: connected.' if message.get('connected') else 'Download account: reconnect in Settings → Connections.'
        return 'Download account: could not verify the saved session. Reconnect in Settings → Connections.'
    except (OSError, subprocess.TimeoutExpired):
        return 'Download account: connection check timed out or could not start. Try again in Settings.'
