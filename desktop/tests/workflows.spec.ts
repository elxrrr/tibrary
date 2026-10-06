import { test, expect, Page, Locator } from "@playwright/test";
import { spawn, execFileSync } from "node:child_process";
import { mkdtempSync, rmSync, realpathSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { resolve, join } from "node:path";
import { createInterface } from "node:readline";
let child: any, folder: string;
let serial = 0;
const pending = new Map<number, (v: any) => void>();
async function rpc(method: string, args: any = {}) {
  return new Promise<any>((resolve) => {
    const id = ++serial;
    pending.set(id, resolve);
    child.stdin.write(JSON.stringify({ id, method, args }) + "\n");
  });
}
// Column menus use real dataset values, including rows outside the current page.
async function columnMenu(page: Page, label: string): Promise<Locator> {
  await page.getByRole("button", {name: `Filter ${label}`, exact: true}).click();
  const menu = page.getByRole("menu", {name: `${label} column options`, exact: true});
  await expect(menu).toBeVisible();
  return menu;
}
function columnValue(menu: Locator, value: string): Locator {
  const escaped = value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  return menu.getByRole("menuitemcheckbox").filter({hasText: new RegExp(`^${escaped}(?:\\s*[\\d,]+)?$`)});
}
async function columnOnly(page: Page, label: string, values: string[]) {
  const menu = await columnMenu(page, label);
  // Wait for the full facet list before an empty include set changes the table.
  for (const value of values) await expect(columnValue(menu, value)).toBeVisible();
  await menu.getByRole("menuitem", {name: "Clear selection", exact: true}).click();
  for (const value of values) {
    await columnValue(menu, value).click();
    await expect(columnValue(menu, value)).toHaveAttribute("aria-checked", "true");
  }
  await menu.press("Escape");
}
async function columnAll(page: Page, label: string) {
  const menu = await columnMenu(page, label);
  await menu.getByRole("menuitem", {name: "Select all", exact: true}).click();
  await menu.press("Escape");
}
async function viewOption(page: Page, label: string, value: string) {
  await page.getByRole("button", {name: "View options", exact: true}).click();
  const dialog = page.getByRole("dialog", {name: "Table view options", exact: true});
  await dialog.getByRole("radiogroup", {name: label, exact: true}).getByRole("radio", {name: value, exact: true}).check();
  await dialog.getByRole("button", {name: "Close view options", exact: true}).click();
}
async function actionOption(page: Page, trigger: string, option: string, role: "menuitem" | "menuitemradio" = "menuitem") {
  await page.getByRole("button", {name: trigger, exact: true}).click();
  await page.getByRole(role, {name: option, exact: true}).click();
}
async function expandLocalArtist(page: Page, artist = "North Assembly") {
  const expander = page.getByRole("button", {name: new RegExp(`^(Expand|Collapse) ${artist.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}$`)});
  await expect(expander).toBeVisible();
  if (await expander.getAttribute("aria-expanded") !== "true") await expander.click();
}
async function localTrackRow(page: Page, title: string, release = "Blue Hours", artist = "North Assembly"): Promise<Locator> {
  await expandLocalArtist(page, artist);
  return localFileRow(page, title, release);
}
async function expandLocalRelease(page: Page, release = "Blue Hours") {
  const expander = page.getByRole("button", {name: new RegExp(`^(Expand|Collapse) ${release.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}$`)});
  await expect(expander).toBeVisible();
  if (await expander.getAttribute("aria-expanded") !== "true") await expander.click();
}
async function localFileRow(page: Page, title: string, release = "Blue Hours"): Promise<Locator> {
  await expandLocalRelease(page, release);
  return page.locator("tbody tr").filter({has: page.getByRole("checkbox", {name: `Select track ${title}`, exact: true})});
}
async function expectTrackPositionInTracks(page: Page, title: string, position: string) {
  const row = page.locator("tbody tr").filter({has: page.getByRole("checkbox", {name: `Select track ${title}`, exact: true})});
  await expect(row.locator("td").nth(1)).toHaveText(title);
  const positionCell = row.getByRole("cell", {name: position, exact: true});
  await expect(positionCell).toBeVisible();
  const header = page.locator("thead th").filter({has: page.getByRole("button", {name: "Tracks", exact: true})});
  const [cellBox, headerBox] = await Promise.all([positionCell.boundingBox(), header.boundingBox()]);
  expect(cellBox!.x).toBeCloseTo(headerBox!.x, 0);
}
function displayColumnValue(value: any): string {
  return value == null ? "—" : typeof value === "object" ? Array.isArray(value)
    ? value.map(displayColumnValue).join(" · ")
    : Object.entries(value).map(([key, item]) => `${key.replaceAll("_", " ")}: ${displayColumnValue(item)}`).join(" · ") : String(value);
}
function filteredMockRows(rows: any[], args: any, skip?: string): any[] {
  return rows.filter(row => Object.entries(args.column_filters || {}).every(([key, selection]: [string, any]) => {
    if (key === skip) return true;
    const value = displayColumnValue(row[key]);
    return (selection.include === undefined || selection.include.includes(value)) && !selection.exclude?.includes(value);
  }));
}
function mockFacets(rows: any[], args: any) {
  const counts = new Map<string, number>();
  for (const row of filteredMockRows(rows, args, args.column)) {
    const value = displayColumnValue(row[args.column]);
    if (args.facet_search && !value.toLowerCase().includes(args.facet_search.toLowerCase())) continue;
    counts.set(value, (counts.get(value) || 0) + 1);
  }
  const options = [...counts].sort(([a], [b]) => a.localeCompare(b)).map(([value, count]) => ({value, label: value, count}));
  return {options: options.slice(0, args.limit || 100), total: options.length};
}
test.beforeEach(async ({ page }) => {
  folder = realpathSync(mkdtempSync(join(tmpdir(), "tibrary-browser-")));
  const root = resolve("..");
  execFileSync(
    "python3",
    [join(root, "desktop/tests/seed_desktop.py"), folder],
    {
      env: {
        ...process.env,
        PYTHONPATH: join(root, "desktop/tests"),
        // Screenshot fixtures have extra tracks; keep functional tests isolated.
        TIBRARY_SCREENSHOTS: test.info().title === "all workflow routes render with no runtime errors" ? process.env.TIBRARY_SCREENSHOTS || "" : "",
      },
    },
  );
  if (test.info().title.startsWith("artist context menu opens")) {
  execFileSync("python3", ["-c", `import sqlite3,json,sys
c=sqlite3.connect(sys.argv[1])
c.execute("CREATE TABLE IF NOT EXISTS match_reviews(artist TEXT PRIMARY KEY,status TEXT,payload TEXT,error TEXT,updated TEXT)")
c.execute("UPDATE mappings SET tidal_id=NULL,status='review'")
for artist, in c.execute("SELECT artist FROM mappings").fetchall():
 c.execute("INSERT OR REPLACE INTO match_reviews(artist,payload) VALUES(?,?)",(artist,json.dumps({"candidates":[{"artist":{"id":"900001","name":"North Assembly"},"evidence":"Saved candidate"}]})))
c.commit()`, join(folder,"db")]);
  }
  if (test.info().title.startsWith("link releases group local files") || test.info().title.startsWith("artist hierarchy") || test.info().title.startsWith("local actions group")) {
    // Create extra real files before the disposable backend opens its database.
    execFileSync("python3", ["-c", `import json,sqlite3,sys
from pathlib import Path
from seed_desktop import write_flac
library=Path(sys.argv[2]); c=sqlite3.connect(sys.argv[1])
for disc,title,track_id in [(1,'Signal','91000200'),(2,'Afterimage','91000201')]:
 path=library/'North Assembly'/'Night Maps (2022)'/f'Disc {disc}'/f'{title}.flac'
 write_flac(path,'North Assembly','Night Maps',title,1,1)
 path.write_bytes(path.read_bytes().replace(b'DISCNUMBER=1/1',f'DISCNUMBER={disc}/2'.encode()))
 metadata={'albumartist':['North Assembly'],'artist':['North Assembly'],'album':['Night Maps'],'title':[title],'tracknumber':['1/1'],'discnumber':[f'{disc}/2'],'date':['2022-09-16'],'duration':180.0,'tidal_album_id':'910002','tidal_track_id':track_id}
 c.execute('INSERT INTO local_files VALUES (?,?,?,?,?,NULL,1)',(str(path),str(library),path.stat().st_size,path.stat().st_mtime_ns,json.dumps(metadata)))
c.commit()`, join(folder,"db"), join(folder,"music")], {
      env:{...process.env,PYTHONPATH:join(root,"desktop/tests")},
    });
  }
  if (test.info().title.startsWith("artist hierarchy")) {
    execFileSync("python3", ["-c", `import json,sqlite3,sys
from pathlib import Path
from seed_desktop import write_flac
library=Path(sys.argv[2]); c=sqlite3.connect(sys.argv[1])
for artist,performer,album,title,track,total in [('Various Artists','First Performer','Night Sessions','Opening',1,2),('Various Artists','Second Performer','Night Sessions','Closing',2,2),('New Local','New Local','Home Tape','Sketch',1,1)]:
 path=library/artist/f'{album} (2024)'/f'{title}.flac'
 write_flac(path,artist,album,title,track,total)
 metadata={'albumartist':[artist],'artist':[performer],'album':[album],'title':[title],'tracknumber':[f'{track}/{total}'],'discnumber':['1/1'],'date':['2024-01-05'],'duration':180.0}
 c.execute('INSERT INTO local_files VALUES (?,?,?,?,?,NULL,1)',(str(path),str(library),path.stat().st_size,path.stat().st_mtime_ns,json.dumps(metadata)))
c.execute('CREATE TABLE IF NOT EXISTS favourite_artists(cache_id TEXT PRIMARY KEY,payload TEXT,fetched TEXT)')
c.execute('INSERT INTO favourite_artists VALUES(?,?,?)',('test',json.dumps([{'id':'900001','name':'North Assembly Online'},{'id':'900099','name':'Remote Favourite'}]),'2026-10-05'))
c.commit()`, join(folder,"db"), join(folder,"music")],{
      env:{...process.env,PYTHONPATH:join(root,"desktop/tests")},
    });
  }
  if(test.info().title.startsWith("online replacements group")) {
    execFileSync("python3",["-c",`import json,sqlite3,sys
from pathlib import Path
from seed_desktop import write_flac
library=Path(sys.argv[2]); c=sqlite3.connect(sys.argv[1])
for album,title,ident in [('First Light','First Light','91000300'),('Signal','Signal','91000200')]:
 path=library/'North Assembly'/f'{album} (2019)'/f'{title}.flac'
 write_flac(path,'North Assembly',album,title,1,1)
 metadata={'albumartist':['North Assembly'],'artist':['North Assembly'],'album':[album],'title':[title],'tracknumber':['1/1'],'discnumber':['1/1'],'date':['2019-01-01'],'duration':180.0,'tidal_track_id':ident}
 c.execute('INSERT INTO local_files VALUES (?,?,?,?,?,NULL,1)',(str(path),str(library),path.stat().st_size,path.stat().st_mtime_ns,json.dumps(metadata)))
plans=[]
for album,path,target,ident,date,tracks,matched,target_tracks in [('Blue Hours',library,'Blue Hours (Deluxe)','910003','2023-04-03',2,['91000300','91000301'],4),('First Light',library/'North Assembly'/'First Light (2019)','Blue Hours (Deluxe)','910003','2023-04-03',1,['91000300'],4),('Signal',library/'North Assembly'/'Signal (2019)','Night Maps','910002','2022-09-16',1,['91000200'],3)]:
 plans.append({'id':f'{ident}::{path}','artist':'North Assembly','release':album,'target':target,'online_id':ident,'path':str(path),'date':'2019-01-01','target_artist':'North Assembly','target_date':date,'target_tracks':target_tracks,'matched_track_ids':matched,'tracks':tracks,'duplicates':tracks,'gained':target_tracks-tracks,'evidence':f'All {tracks} local recordings are contained in {target}; matching recording identifiers, mix and duration','status':'Larger online release','affected':True})
c.execute('INSERT OR REPLACE INTO app_preferences VALUES(?,?)',(f'desktop-online:{library}',json.dumps(plans)))
c.commit()`,join(folder,"db"),join(folder,"music")],{
      env:{...process.env,PYTHONPATH:join(root,"desktop/tests")},
    });
  }
  child = spawn(
    join(root, "desktop/src-tauri/target/debug/tibrary"),
    ["--rpc", "--db", join(folder, "db")],
    { env: { ...process.env, TIBRARY_TEST_MODE: "1" } },
  );
  createInterface({ input: child.stdout }).on("line", (line) => {
    const v = JSON.parse(line);
    if (v.id) {
      pending.get(v.id)?.(v);
      pending.delete(v.id);
    }
  });
  await expect
    .poll(async () => {
      const s = await rpc("state");
      return s.result?.job?.status;
    })
    .toBe("complete");
  await page.route("**/__test_rpc", async (route) => {
    const r = route.request().postDataJSON();
    await route.fulfill({ json: await rpc(r.method, r.args) });
  });
});
test.afterEach(async ({ page }) => {
  // Retire intercepted requests before closing the disposable backend. A quick
  // startup test can end with an independent dashboard read still in flight;
  // only replies arriving during teardown are ignored after the page closes.
  await page.unrouteAll({behavior:"ignoreErrors"});
  await page.close();
  child.stdin.end();
  await new Promise<void>((resolve) => child.on("exit", () => resolve()));
  rmSync(folder, { recursive: true, force: true });
});

test("tables publish usable results while catalogue revisions keep advancing", async ({page}) => {
  await page.addInitScript(() => {
    let id=0;
    const callbacks=new Map<number,(event:any)=>void>(), listeners=new Map<number,string>();
    (window as any).__TAURI_INTERNALS__={
      transformCallback:(callback:(event:any)=>void)=>{callbacks.set(++id,callback);return id;},
      invoke:async(command:string,args:any)=>{
        if(command==="plugin:event|listen") {listeners.set(args.handler,args.event);return args.handler;}
        if(command==="plugin:event|unlisten") callbacks.delete(args.eventId);
      },
    };
    (window as any).__TAURI_EVENT_PLUGIN_INTERNALS__={unregisterListener:(_event:string,eventId:number)=>listeners.delete(eventId)};
    (window as any).emitBackendEvent=(payload:any)=>{
      for(const [handler,event] of listeners) if(event==="backend-event") callbacks.get(handler)?.({event,id:handler,payload});
    };
  });
  let revision=1000;
  const job={id:"advancing-refresh",kind:"discography",status:"running",message:"Checking cached artist releases",started:Date.now()/1000,completed:1,total:20};
  await page.route("**/__test_rpc",async route=>{
    const request=route.request().postDataJSON();
    const response=await rpc(request.method,request.args);
    if(["state","job.status"].includes(request.method) && response.result) {
      response.result.online_job=job;
      if(request.method==="state") response.result.revision=++revision;
      else delete response.result.revision;
    }
    if(request.method==="table" && request.args.route==="missing") await new Promise(resolve=>setTimeout(resolve,700));
    await route.fulfill({json:response});
  });
  await page.goto("/");
  await expect(page.getByRole("main",{name:"Overview",exact:true})).toBeVisible();
  await page.locator("aside").getByRole("button",{name:"Missing releases",exact:true}).click();
  await viewOption(page, "Release timeline", "All missing releases");
  await page.evaluate(()=>{(window as any).refreshTestTimer=setInterval(()=>(window as any).emitBackendEvent({event:"changed"}),80);});
  try {
    await expect(page.locator("tbody")).toContainText("Night Maps",{timeout:1800});
    await expect(page.getByRole("button",{name:"Update missing releases",exact:true})).toBeDisabled();
    await page.locator("aside").getByRole("button",{name:"Download queue",exact:true}).click();
    await expect(page.getByRole("checkbox",{name:"Select Blue Hours",exact:true})).toBeVisible({timeout:1500});
    await expect(page.getByRole("checkbox",{name:"Select Blue Hours",exact:true})).toBeEnabled();
  } finally {await page.evaluate(()=>clearInterval((window as any).refreshTestTimer));}
  await expect(page.getByRole("alert")).toHaveCount(0);
});

test("queue approvals and lazy tracks stay usable during independent scans and refreshes", async ({page}) => {
  const started=Date.now()/1000;
  const local={id:"independent-scan",kind:"scan",status:"running",message:"Reading local files",started,completed:1,total:20};
  const online={id:"independent-refresh",kind:"discography",status:"running",message:"Checking artist releases",started,completed:1,total:20};
  let tracksReady=false, details=0;
  await page.route("**/__test_rpc",async route=>{
    const request=route.request().postDataJSON();
    const response=await rpc(request.method,request.args);
    if(["state","job.status"].includes(request.method) && response.result) Object.assign(response.result,{job:local,online_job:online,download_job:null});
    if(request.method==="table" && request.args.route==="queue" && !tracksReady) {
      const row=response.result.rows.find((row:any)=>row.release==="Blue Hours");
      if(row) {row.children=[];row.expanded_available=false;}
    }
    if(request.method==="release.ensure_tracks") {
      details++;
      await new Promise(resolve=>setTimeout(resolve,250));
      tracksReady=true;
    }
    await route.fulfill({json:response});
  });
  await page.goto("/");
  await page.locator("aside").getByRole("button",{name:"Download queue",exact:true}).click();
  await page.getByRole("checkbox",{name:"Select Blue Hours",exact:true}).check();
  await expect(page.getByRole("checkbox",{name:"Select Blue Hours",exact:true})).toBeChecked();
  await page.getByRole("button",{name:"Expand Blue Hours",exact:true}).click();
  await expect(page.getByRole("status").filter({hasText:"Loading track details…"})).toBeVisible();
  await expect(page.getByRole("checkbox",{name:"Select track First Light",exact:true})).toBeVisible();
  expect(details).toBe(1);
  await page.getByRole("checkbox",{name:"Select track First Light",exact:true}).uncheck();
  await expect(page.getByRole("checkbox",{name:"Select track First Light",exact:true})).not.toBeChecked();
  await expect(page.getByRole("checkbox",{name:"Select Blue Hours",exact:true})).toBeChecked({indeterminate:true});
  await page.getByRole("button",{name:"Download",exact:true}).click();
  const dialog=page.getByRole("dialog");
  await expect(dialog.getByRole("table",{name:"Tracks in Blue Hours"})).toContainText("Drift");
  await expect(dialog).not.toContainText("First Light");
  await expect(dialog.getByRole("button",{name:"Start download",exact:true})).toBeEnabled();
  await dialog.getByRole("button",{name:"Cancel",exact:true}).click();
  await actionOption(page, "Download options", "Export");
  await expect(page.getByRole("dialog")).toContainText("tidal.com/track/");
  await expect(page.getByRole("alert")).toHaveCount(0);
});

test("all workflow routes render with no runtime errors", async ({ page }) => {
  if (process.env.TIBRARY_SCREENSHOTS) {
    await page.emulateMedia({colorScheme:"dark"});
    await rpc("settings.save", {section:"downloads",values:{output:"/Users/demo/Music/Tibrary"}});
    await rpc("queue.decision", {ids:["910002","910003","910004","910005","910006"],decision:"removed"});
  }
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await page.goto("/");
  await expect(
    page.getByRole("main", { name: "Overview", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Working", exact: true }),
  ).toHaveCount(0);
  const screenshots: Record<string, string> = {"Prepare library":"prepare_library", "Link artists":"link_artists", "Link releases":"link_releases", "Missing releases":"missing_releases", "Local duplicates":"local_duplicates", "MQA audit":"mqa_audit", "Favourite artists":"favourite_artists", "Activity":"activity_log", "General":"general_settings"};
  if (process.env.TIBRARY_SCREENSHOTS) {
    await expect(page.getByRole("region",{name:"Latest missing releases"}).locator(".library-row").first()).toBeVisible();
    await page.screenshot({path:"docs/imgs/overview.png"});
  }
  const categoryPages: Record<string,string> = {"Prepare library":"Correct tags", "Link catalogue":"Link artists", "Complete library":"Missing releases", "Update library":"MQA audit", "Settings":"General"};
  for (const name of [
    "Prepare library",
    "Correct tags",
    "Organise files",
    "MQA audit",
    "Local duplicates",
    "Link catalogue",
    "Link artists",
    "Link releases",
    "Favourite artists",
    "Update library",
    "Add missing tags",
    "Fix artwork",
    "Online replacements",
    "Complete library",
    "Missing releases",
    "Download queue",
    "Downloaded releases",
    "Settings",
    "General",
    "Activity",
  ]) {
    await page
      .locator("aside")
      .getByRole("button", { name, exact: true })
      .click();
    await expect(
      page.getByRole("main", { name:categoryPages[name] || name, exact: true }),
    ).toBeVisible();
    await expect(page.locator("main h1")).toHaveCount(0);
    await expect(page.locator("aside .sidebar-bottom").getByRole("combobox", {name:"Active library",exact:true})).toBeVisible();
    if (await page.locator(".table-scroll").count()) {
      await expect(page.locator(".table-scroll")).toHaveAttribute("aria-busy", "false");
      const headers=page.locator(".table-header-actions");
      let filterCount=0;
      for(let index=0;index<await headers.count();index++) {
        const header=headers.nth(index), label=(await header.getByRole("button").first().innerText()).trim();
        const numerical=["Tracks","Local tracks","Tracks gained","Duplicates","Duplicate files","Disc · track","Online ID","Online IDs"].includes(label)
          || (["Link artists","Favourite artists","Link catalogue"].includes(name) && label === "Releases");
        const filterable=!numerical && label !== "Evidence";
        await expect(header.getByRole("button",{name:`Filter ${label}`,exact:true})).toHaveCount(filterable ? 1 : 0);
        if(filterable) filterCount++;
      }
      await expect(page.getByRole("button",{name:/^Filter /})).toHaveCount(filterCount);
      await expect(page.getByRole("combobox",{name:"Table filter",exact:true})).toHaveCount(0);
      const pagination=page.getByRole("navigation",{name:"Table pagination",exact:true});
      const actions=page.getByRole("group",{name:"Table actions",exact:true});
      await expect(pagination).toBeVisible();
      await expect(actions).toBeVisible();
      const tableBox=(await page.locator(".table-scroll").boundingBox())!;
      const paginationBox=(await pagination.boundingBox())!, actionsBox=(await actions.boundingBox())!;
      expect(paginationBox.y+paginationBox.height).toBeLessThanOrEqual(tableBox.y+1);
      expect(actionsBox.y).toBeGreaterThanOrEqual(tableBox.y+tableBox.height-1);
      await expect(page.locator(".action-grid, .table-footer")).toHaveCount(0);
    }
    if (name === "MQA audit") await expect(page.getByRole("button",{name:"Scan selected tracks",exact:true})).toBeEnabled();
    await expect(page.getByRole("alert")).toHaveCount(0);
    if (process.env.TIBRARY_SCREENSHOTS && screenshots[name]) {
      if (name === "Prepare library") {
        await actionOption(page,"Choose correction","Track & disc numbers","menuitemradio");
        await expandLocalRelease(page);
        await expect(page.getByRole("checkbox",{name:"Select track First Light",exact:true})).toBeVisible();
      }
      if (name === "Link releases") await columnAll(page,"Status");
      if (["Link artists","Favourite artists","Link releases"].includes(name)) {
        const artist=page.getByRole("button",{name:"Expand North Assembly",exact:true});
        if (await artist.count()) await artist.click();
      }
      if (name === "Link releases") {
        const remaining=page.locator("tbody").getByRole("button",{name:/^Expand /});
        for(let count=0;count<20 && await remaining.count();count++) await remaining.first().click();
        await expect(page.getByRole("checkbox",{name:"Select track Low Tide",exact:true})).toBeVisible();
      }
      if (name === "Local duplicates") {
        await page.getByRole("button", {name:"Check local duplicates",exact:true}).click();
        await expect(page.locator(".header-workload")).toBeVisible();
        await expect.poll(async () => (await rpc("job.status")).result?.job?.status).toBe("complete");
        await expect(page.getByRole("button", {name:"Check local duplicates",exact:true})).toBeEnabled();
        await expect(page.locator(".header-workload")).toHaveCount(0);
      }
      await page.mouse.move(1590, 10);
      // Let the pointer leave the sidebar before capturing hover transitions.
      await page.waitForTimeout(120);
      await page.screenshot({path:`docs/imgs/${screenshots[name]}.png`});
    }
  }
  if (process.env.TIBRARY_SCREENSHOTS) {
    await page.locator("aside").getByRole("button", {name:"General",exact:true}).click();
    await page.getByLabel("Colour theme").selectOption("light");
    const native=(await rpc("appearance.accent")).result;
    await expect(page.locator("html")).toHaveAttribute("data-theme","light");
    await expect.poll(()=>page.locator("html").evaluate(element=>getComputedStyle(element).getPropertyValue("--system-blue").trim())).toBe(native.palettes.light.blue);
    await page.mouse.move(1590,10);
    await page.waitForTimeout(120);
    await page.screenshot({path:"/tmp/tibrary-general-light-0.9.21.png"});
    await page.locator("aside").getByRole("button", {name:"MQA audit",exact:true}).click();
    await expandLocalRelease(page);
    await expect(page.getByRole("checkbox",{name:"Select track First Light",exact:true})).toBeVisible();
    await expect(page.getByRole("button",{name:"Scan selected tracks",exact:true})).toBeEnabled();
    await page.mouse.move(1590,10);
    await page.waitForTimeout(120);
    await page.screenshot({path:"/tmp/tibrary-mqa-light-0.9.21.png"});
  }
  await page.setViewportSize({width:980,height:680});
  await page.locator("aside").getByRole("button",{name:"Missing releases",exact:true}).click();
  const updateOptions=page.getByRole("button",{name:"Release update options",exact:true});
  await updateOptions.click();
  const updateMenu=page.getByRole("menu",{name:"Release update options",exact:true});
  await expect(updateMenu).toBeVisible();
  const menuBox=(await updateMenu.boundingBox())!, triggerBox=(await updateOptions.boundingBox())!;
  expect(menuBox.y+menuBox.height).toBeLessThanOrEqual(triggerBox.y);
  expect(menuBox.x).toBeGreaterThanOrEqual(0);
  expect(menuBox.x+menuBox.width).toBeLessThanOrEqual(980);
  for (const key of ["Home","End"] as const) {
    await updateMenu.press(key);
    const item=key === "Home" ? updateMenu.locator("button:not(:disabled)").first() : updateMenu.locator("button:not(:disabled)").last();
    await expect(item).toBeFocused();
    const itemBox=(await item.boundingBox())!;
    expect(itemBox.y).toBeGreaterThanOrEqual(menuBox.y);
    expect(itemBox.y+itemBox.height).toBeLessThanOrEqual(menuBox.y+menuBox.height);
    expect(await page.evaluate(()=>window.scrollY)).toBe(0);
  }
  await updateMenu.press("Escape");
  await expect(updateOptions).toBeFocused();
  await expect(updateMenu).toHaveCount(0);
  expect(errors).toEqual([]);
});

test("window navigation remains available with the sidebar hidden and overview lists stay bounded", async ({page}) => {
  await rpc("queue.decision", {ids:["910002","910003","910004","910005","910006"],decision:"removed"});
  await page.goto("/");
  for(const width of [1600,1000]) {
    await page.setViewportSize({width,height:1000});
    const boundary=await page.locator(".drag-region").evaluate(element=>{
      const before=getComputedStyle(element,"::before"), sidebar=document.querySelector("aside")!.getBoundingClientRect();
      return {sizing:before.boxSizing,sidebar:sidebar.width,
        width:parseFloat(before.width)+(before.boxSizing === "border-box" ? 0 : parseFloat(before.borderRightWidth))};
    });
    expect(boundary.sizing).toBe("border-box");
    expect(boundary.width).toBeCloseTo(boundary.sidebar,4);
  }
  await page.setViewportSize({width:1600,height:1000});
  await expect(page.getByRole("button", {name:"Back",exact:true})).toBeDisabled();
  await page.locator("aside").getByRole("button", {name:"Prepare library",exact:true}).click();
  await page.locator("aside").getByRole("button", {name:"Link catalogue",exact:true}).click();
  await page.getByRole("button", {name:"Hide sidebar",exact:true}).click();
  await expect(page.locator("aside")).toBeHidden();
  await expect(page.getByRole("combobox",{name:"Active library",exact:true})).toBeHidden();
  expect(await page.locator(".drag-region").evaluate(element=>getComputedStyle(element,"::before").borderRightWidth)).toBe("0px");
  expect((await page.locator("main").boundingBox())!.x).toBe(0);
  await page.getByRole("button", {name:"Back",exact:true}).click();
  await expect(page.getByRole("main", {name:"Correct tags",exact:true})).toBeVisible();
  await page.getByRole("button", {name:"Forward",exact:true}).click();
  await expect(page.getByRole("main", {name:"Link artists",exact:true})).toBeVisible();
  await page.getByRole("button", {name:"Back",exact:true}).click();
  await page.getByRole("button", {name:"Show sidebar",exact:true}).click();
  const librarySelect=page.locator("aside .sidebar-bottom").getByRole("combobox",{name:"Active library",exact:true});
  await expect(librarySelect).toHaveValue(join(folder,"music"));
  await expect(page.locator(".sidebar-version")).toBeVisible();
  const [selectBox,versionBox,footerBox]=await Promise.all([librarySelect.boundingBox(),page.locator(".sidebar-version").boundingBox(),page.locator(".sidebar-bottom").boundingBox()]);
  expect(selectBox!.y).toBeGreaterThanOrEqual(footerBox!.y);
  expect(versionBox!.y+versionBox!.height).toBeLessThanOrEqual(footerBox!.y+footerBox!.height);
  expect(footerBox!.y+footerBox!.height).toBeGreaterThan(970);
  const toolbar=(await page.locator("main .filters").boundingBox())!;
  expect(toolbar.y).toBeLessThan(95);
  await page.locator("aside").getByRole("button", {name:"Overview",exact:true}).click();
  await expect(page.getByRole("button", {name:"Forward",exact:true})).toBeDisabled();
  const list = page.getByRole("region", {name:"Latest missing releases"});
  await expect(list.locator(".library-row").first()).toBeVisible();
  const listBox=(await list.boundingBox())!, mainBox=(await page.locator("main").boundingBox())!;
  expect(listBox.height).toBeGreaterThan(320);
  expect(listBox.y+listBox.height).toBeLessThanOrEqual(mainBox.y+mainBox.height);
  expect(await list.evaluate(element => getComputedStyle(element).overflowY)).toBe("auto");
  expect(await page.locator("main").evaluate(element=>element.scrollHeight <= element.clientHeight+1)).toBe(true);
  expect(await page.evaluate(()=>window.scrollY)).toBe(0);
  const fades=page.locator(".latest-missing-scroll > .edge-gradient-top, .latest-missing-scroll > .edge-gradient-bottom");
  await expect(fades).toHaveCount(2);
  for(const theme of ["light","dark"]) {
    await page.evaluate(theme=>{document.documentElement.dataset.theme=theme;},theme);
    const styles=await fades.evaluateAll(elements=>elements.map(element=>{
      const style=getComputedStyle(element),card=getComputedStyle(element.closest(".card")!);
      return {hidden:element.getAttribute("aria-hidden"),position:style.position,pointerEvents:style.pointerEvents,background:style.backgroundImage,card:card.backgroundColor};
    }));
    for(const style of styles) {
      expect(style.hidden).toBe("true");
      expect(style.position).toBe("absolute");
      expect(style.pointerEvents).toBe("none");
      expect(style.background).toContain("linear-gradient");
      expect(style.background).toContain(style.card);
    }
  }
  // A shorter window makes the real cached catalogue overflow this region.
  await page.setViewportSize({width:1600,height:680});
  expect(await list.evaluate(element=>element.scrollHeight>element.clientHeight)).toBe(true);
  await list.evaluate(element=>{element.scrollTop=element.scrollHeight;});
  await expect.poll(()=>list.evaluate(element=>element.scrollTop)).toBeGreaterThan(0);
  expect(await page.locator("main").evaluate(element=>element.scrollTop)).toBe(0);
  expect(await page.evaluate(()=>window.scrollY)).toBe(0);
  const last=list.locator(".library-row").last(),web=last.getByRole("button",{name:"Open on web",exact:true});
  await expect(web).toBeInViewport();
  const [buttonBox,fadeBox]=await Promise.all([web.boundingBox(),page.locator(".latest-missing-scroll > .edge-gradient-bottom").boundingBox()]);
  expect(buttonBox!.y+buttonBox!.height).toBeLessThanOrEqual(fadeBox!.y+1);
  await page.evaluate(()=>{(window as any).openedURLs=[];(window as any).__TAURI_INTERNALS__={invoke:async(command:string,args:any)=>{if(command==="open_external") (window as any).openedURLs.push(args.url);}};});
  await web.click();
  await expect.poll(()=>page.evaluate(()=>(window as any).openedURLs)).toHaveLength(1);
  expect((await page.evaluate(()=>(window as any).openedURLs))[0]).toMatch(/^https:\/\/tidal\.com\/album\/\d+$/);
  await expect(page.getByRole("alert")).toHaveCount(0);
});
test("local table sorting and filters are usable", async ({ page }) => {
  await page.goto("/");
  await page
    .locator("aside")
    .getByRole("button", { name: "Link releases", exact: true })
    .click();
  await columnAll(page, "Status");
  await expect(page.locator("tbody tr").first()).toBeVisible();
  await page.getByRole("button", { name: "Release", exact: true }).click();
  await page.getByRole("button", { name: "Release", exact: true }).click();
});
test("numeric and evidence headers keep sorting without value filters or facet requests", async ({page}) => {
  const facets:string[]=[];
  await page.route("**/__test_rpc",async route=>{
    const request=route.request().postDataJSON();
    if(request.method==="table.facets") facets.push(`${request.args.route}:${request.args.column}`);
    await route.fulfill({json:await rpc(request.method,request.args)});
  });
  await page.goto("/");
  await page.locator("aside").getByRole("button",{name:"Link artists",exact:true}).click();
  for(const label of ["Tracks","Releases","Evidence"]) {
    await expect(page.getByRole("button",{name:`Filter ${label}`,exact:true})).toHaveCount(0);
    const header=page.getByRole("columnheader").filter({has:page.getByRole("button",{name:label,exact:true})});
    await header.click({button:"right"});
    const menu=page.getByRole("menu",{name:`${label} column options`,exact:true});
    await expect(menu.getByRole("menuitemcheckbox")).toHaveCount(0);
    await menu.getByRole("menuitemradio",{name:"Sort descending",exact:true}).click();
    await expect(header).toHaveAttribute("aria-sort","descending");
    await header.getByRole("button",{name:label,exact:true}).click();
    await expect(header).toHaveAttribute("aria-sort","ascending");
  }
  await columnOnly(page,"Match status",["Confirmed"]);
  await expect(page.locator("tbody tr")).toHaveCount(1);
  await page.locator("aside").getByRole("button",{name:"Link releases",exact:true}).click();
  await columnAll(page,"Status");
  await expect(page.getByRole("button",{name:"Filter Tracks",exact:true})).toHaveCount(0);
  await expect(page.getByRole("button",{name:"Filter Evidence",exact:true})).toHaveCount(0);
  await columnOnly(page,"Release",["Blue Hours"]);
  await expandLocalArtist(page);
  await expect(page.getByRole("button",{name:"Expand Blue Hours",exact:true})).toBeVisible();
  expect(facets.some(key=>/:(tracks|evidence|position|gained|duplicates|online_id)$/.test(key))).toBe(false);
  expect(facets).not.toContain("artists:release");
  await expect(page.getByRole("alert")).toHaveCount(0);
});
test("queue approvals cascade, persist and survive sorting and expansion", async ({
  page,
}) => {
  await page.goto("/");
  await page
    .locator("aside")
    .getByRole("button", { name: "Download queue", exact: true })
    .click();
  const parent = page.getByRole("checkbox", {
    name: "Select Blue Hours",
    exact: true,
  });
  await parent.check();
  await expect(parent).toBeChecked();
  await page
    .getByRole("button", { name: "Expand Blue Hours", exact: true })
    .click();
  const childBox = page.getByRole("checkbox", {
    name: "Select track First Light",
    exact: true,
  });
  await expect(childBox).toBeChecked();
  await expectTrackPositionInTracks(page, "First Light", "1 · 1");
  if (process.env.TIBRARY_SCREENSHOTS) await page.screenshot({path: "/tmp/tibrary-expanded-queue-0.9.17.png"});
  await childBox.uncheck();
  await expect(parent).toHaveJSProperty("indeterminate", true);
  await expect.poll(async () => (await rpc("queue.export",{format:"text"})).result?.text)
    .toBe("https://tidal.com/track/91000101\nhttps://tidal.com/track/91000102\n");
  await actionOption(page, "Download options", "Export");
  await expect(page.getByRole("textbox",{name:"Download links"})).toHaveValue("https://tidal.com/track/91000101\nhttps://tidal.com/track/91000102\n");
  await expect(page.getByRole("dialog")).not.toContainText("JSON");
  await page.getByRole("button",{name:"Close",exact:true}).click();
  await page.getByRole("button", { name: "Coverage", exact: true }).click();
  await expect(childBox).toBeVisible();
  await expect(childBox).not.toBeChecked();
  await parent.check();
  await expect(childBox).toBeChecked();
  await expect.poll(async () => (await rpc("queue.export",{format:"text"})).result?.text)
    .toBe("https://tidal.com/album/910001\n");
  await parent.uncheck();
  await expect(childBox).not.toBeChecked();
  await page
    .locator("aside")
    .getByRole("button", { name: "Overview", exact: true })
    .click();
  await page
    .locator("aside")
    .getByRole("button", { name: "Download queue", exact: true })
    .click();
  await expect(parent).not.toBeChecked();
  await expect(page.getByRole("alert")).toHaveCount(0);
  // Completed-release choices are transient; exporting them must not alter the
  // original queue approval or expand a single-track choice to the whole album.
  await rpc("queue.decision",{ids:["910001"],decision:"downloaded"});
  await page.locator("aside").getByRole("button",{name:"Downloaded releases",exact:true}).click();
  const downloadedExpander = page.getByRole("button",{name:/^(Expand|Collapse) Blue Hours$/});
  await expect(downloadedExpander).toBeVisible();
  if (await downloadedExpander.getAttribute("aria-expanded") !== "true") await downloadedExpander.click();
  await expectTrackPositionInTracks(page, "First Light", "1 · 1");
  await page.getByRole("checkbox",{name:"Select track First Light",exact:true}).check();
  await page.getByRole("button",{name:"Export",exact:true}).click();
  await expect(page.getByRole("textbox",{name:"Download links"})).toHaveValue("https://tidal.com/track/91000100\n");
});
test("multiple file context action affects only selected files", async ({
  page,
}) => {
  await page.goto("/");
  await page
    .locator("aside")
    .getByRole("button", { name: "Link releases", exact: true })
    .click();
  await columnAll(page, "Status");
  await expect(page.locator("tbody tr").first()).toBeVisible();
  await page.getByRole("checkbox", { name: "Select visible rows" }).check();
  await page.locator("tbody tr").first().click({ button: "right" });
  await page
    .getByRole("menuitem", { name: "Ignore 2 selected tracks" })
    .click();
  const ignoredMenu = await columnMenu(page, "Status");
  await ignoredMenu.getByRole("menuitem", {name:"Clear selection",exact:true}).click();
  await ignoredMenu.press("Escape");
  await expect(page.locator("tbody tr")).toHaveCount(0);
  await columnOnly(page, "Status", ["Ignored"]);
  await expect(page.locator("tbody tr")).toHaveCount(1);
  await expandLocalArtist(page);
  await page.getByRole("button", {name:"Expand Blue Hours",exact:true}).click();
  await expect(page.getByRole("checkbox",{name:"Select track First Light",exact:true})).toBeVisible();
  await expect(page.getByRole("checkbox",{name:"Select track Drift",exact:true})).toBeVisible();
  await expect(page.getByRole("alert")).toHaveCount(0);
});
test("link releases group local files and keep selected track actions scoped", async ({page}) => {
  // A second real-file release spans disc folders. Grouping must retain the
  // whole local release while actions receive only individual file paths.
  const root=join(folder,"music");
  const paths=[join(root,"North Assembly/Night Maps (2022)/Disc 1/Signal.flac"),join(root,"North Assembly/Night Maps (2022)/Disc 2/Afterimage.flac")];
  const args={route:"links",root,group_releases:true,sort:"release",direction:"asc",limit:1};
  const first=(await rpc("table",args)).result, second=(await rpc("table",{...args,offset:1})).result;
  expect(first.total).toBe(2);
  expect(first.rows[0].release).toBe("Blue Hours");
  expect(first.rows[0].children).toHaveLength(2);
  expect(second.rows[0].release).toBe("Night Maps");
  expect(second.rows[0].children.map((row:any)=>row.path).sort()).toEqual(paths.sort());
  const filtered=(await rpc("table",{...args,column_filters:{release:{include:["Night Maps"]}}})).result;
  expect(filtered.total).toBe(1);
  expect(filtered.rows[0].children).toHaveLength(2);
  const facet=(await rpc("table.facets",{...args,column:"release",limit:100})).result;
  expect(facet.options.map((option:any)=>option.value)).toEqual(["Blue Hours","Night Maps"]);
  const requests:any[]=[];
  await page.route("**/__test_rpc",async route=>{
    const request=route.request().postDataJSON();
    if(request.method==="job.start" && request.args.kind==="link") {
      requests.push(request);
      await route.fulfill({json:{result:{id:"group-scope-check",kind:"link",status:"complete",message:"Selected files checked"}}});
      return;
    }
    if(["tracks.ignore","tracks.unlink"].includes(request.method)) requests.push(request);
    await route.fulfill({json:await rpc(request.method,request.args)});
  });
  await page.goto("/");
  await page.locator("aside").getByRole("button",{name:"Link releases",exact:true}).click();
  await columnAll(page,"Status");
  await expect(page.locator("tbody tr")).toHaveCount(1);
  await expect(page.getByRole("navigation",{name:"Table pagination",exact:true})).toContainText("2 releases");
  const artist=page.getByRole("checkbox",{name:"Select North Assembly",exact:true});
  await expandLocalArtist(page);
  await expect(page.locator("tbody tr")).toHaveCount(3);
  const parent=page.getByRole("checkbox",{name:"Select Blue Hours",exact:true});
  await parent.check();
  const firstTrack=await localTrackRow(page,"First Light");
  const firstCheck=firstTrack.getByRole("checkbox",{name:"Select track First Light",exact:true});
  const driftCheck=page.getByRole("checkbox",{name:"Select track Drift",exact:true});
  const localPosition=first.rows[0].children.find((row:any)=>row.title==="First Light").position;
  const trackPosition=firstTrack.getByRole("cell",{name:localPosition,exact:true});
  const tracksHeader=page.getByRole("columnheader").filter({has:page.getByRole("button",{name:"Tracks",exact:true})});
  await expect(trackPosition).toBeVisible();
  expect((await trackPosition.boundingBox())!.x).toBeCloseTo((await tracksHeader.boundingBox())!.x,0);
  await expect(firstTrack.locator("td").nth(2)).toHaveText("First Light");
  await expect(firstCheck).toBeChecked();
  await expect(driftCheck).toBeChecked();
  await firstCheck.uncheck();
  await expect(parent).toHaveJSProperty("indeterminate",true);
  await expect(artist).toHaveJSProperty("indeterminate",true);
  await expect(page.getByRole("checkbox",{name:"Select visible rows",exact:true})).toHaveJSProperty("indeterminate",true);
  await page.getByRole("button",{name:"Release",exact:true}).click();
  await page.getByRole("button",{name:"Release",exact:true}).click();
  await expect(page.getByRole("button",{name:"Collapse Blue Hours",exact:true})).toBeVisible();
  await expect(page.getByRole("button",{name:"Collapse North Assembly",exact:true})).toBeVisible();
  await expect(driftCheck).toBeChecked();
  await expect(firstCheck).not.toBeChecked();
  if(process.env.TIBRARY_SCREENSHOTS) {
    await page.screenshot({path:"/tmp/tibrary-expanded-linked-releases-0.9.19.png"});
  }
  const driftPath=join(root,"Drift.flac");
  await page.getByRole("button",{name:"Recheck 1 selected",exact:true}).click();
  await expect.poll(()=>requests.length).toBe(1);
  expect(requests[0].args.args.ids).toEqual([driftPath]);
  await page.getByRole("button",{name:"Actions for track Drift",exact:true}).click();
  await page.getByRole("menuitem",{name:"Unlink 1 selected tracks",exact:true}).click();
  await expect.poll(()=>requests.length).toBe(2);
  expect(requests[1].args.ids).toEqual([driftPath]);
  await page.getByRole("button",{name:"Clear selection",exact:true}).click();
  await firstCheck.check();
  await page.getByRole("button",{name:"Actions for track First Light",exact:true}).click();
  await page.getByRole("menuitem",{name:"Ignore 1 selected tracks",exact:true}).click();
  await expect.poll(()=>requests.length).toBe(3);
  expect(requests[2].args.ids).toEqual([join(root,"First Light.flac")]);
  await columnOnly(page,"Status",["Ignored"]);
  await expect(page.getByRole("navigation",{name:"Table pagination",exact:true})).toContainText("1–1 of 1");
  await expect(page.getByRole("checkbox",{name:"Select track First Light",exact:true})).toBeVisible();
  await expect(page.getByRole("checkbox",{name:"Select track Drift",exact:true})).toHaveCount(0);
  const ignored=(await rpc("table",{...args,column_filters:{status:{include:["Ignored"]}}})).result;
  expect(ignored.rows[0].children.map((row:any)=>row.path)).toEqual([join(root,"First Light.flac")]);
  expect(requests.every(request=>!(request.args.ids || request.args.args.ids).some((id:string)=>id.startsWith("local-release:")))).toBe(true);
  await expect(page.getByRole("alert")).toHaveCount(0);
});
test("artist hierarchy shows local release evidence without exposing online IDs or track controls", async ({page}) => {
  const root=join(folder,"music"), errors:string[]=[], detailArtists:string[]=[];
  page.on("pageerror",error=>errors.push(error.message));
  await page.route("**/__test_rpc",async route=>{
    const request=route.request().postDataJSON();
    if(request.method==="detail" && request.args.artist) detailArtists.push(request.args.artist);
    await route.fulfill({json:await rpc(request.method,request.args)});
  });
  await page.goto("/");
  const sidebar=page.locator("aside");
  await sidebar.getByRole("button",{name:"Link artists",exact:true}).click();
  await expect(page.getByRole("columnheader",{name:/Online ID/i})).toHaveCount(0);
  await expect(page.getByRole("button",{name:"Evidence",exact:true})).toBeVisible();
  await expect(page.locator("tbody tr")).toHaveCount(2);
  await expect(page.locator("tbody")).not.toContainText("Various Artists");
  await page.getByRole("button",{name:"North Assembly",exact:true}).click();
  const blue=page.locator("tbody tr").filter({has:page.getByRole("button",{name:"Blue Hours",exact:true})});
  const night=page.locator("tbody tr").filter({has:page.getByRole("button",{name:"Night Maps",exact:true})});
  await expect(blue).toBeVisible();
  await expect(night).toBeVisible();
  await expect(blue).toContainText("Linked");
  await expect(blue.locator("input[type=checkbox], button[aria-label^=Actions]")).toHaveCount(0);
  await expect(page.getByRole("button",{name:"Expand Blue Hours",exact:true})).toHaveCount(0);
  await expect(page.getByRole("checkbox",{name:/Select track/})).toHaveCount(0);
  await blue.getByRole("button",{name:"Blue Hours",exact:true}).click();
  const dialog=page.getByRole("dialog");
  await expect(dialog).toContainText("Local release details");
  await expect(dialog).toContainText("Blue Hours");
  await expect(dialog).toContainText("2 local tracks");
  const box=(await dialog.boundingBox())!;
  expect(box.width).toBeGreaterThanOrEqual(1300);
  expect(box.x).toBeGreaterThanOrEqual(0);
  expect(box.y+box.height).toBeLessThanOrEqual(1000);
  await dialog.getByRole("button",{name:"Done",exact:true}).click();
  const catalogue=(await rpc("table",{route:"favourites",root,local_releases:true,limit:100})).result;
  expect(catalogue.total).toBe(3);
  const canonical=catalogue.rows.find((row:any)=>row.artist==="North Assembly Online");
  expect(canonical.children).toHaveLength(2);
  expect(canonical.tracks).toBe(4);
  expect(canonical.lookup_artist).toBe("North Assembly");
  expect(catalogue.rows.some((row:any)=>row.artist==="North Assembly")).toBe(false);
  await sidebar.getByRole("button",{name:"Favourite artists",exact:true}).click();
  await expect(page.getByRole("columnheader",{name:/Online ID/i})).toHaveCount(0);
  await expect(page.locator("tbody tr")).toHaveCount(3);
  const north=page.locator("tbody tr").filter({has:page.getByRole("button",{name:"North Assembly Online",exact:true})});
  const local=page.locator("tbody tr").filter({has:page.getByRole("button",{name:"New Local",exact:true})});
  await expect(north).toContainText("In library");
  await expect(north).toContainText("Linked");
  await expect(local).toContainText("Local only");
  await expect(local).toContainText("Not linked");
  await expandLocalArtist(page,"North Assembly Online");
  await expect(page.getByRole("button",{name:"Blue Hours",exact:true})).toBeVisible();
  await expect(page.getByRole("button",{name:"Night Maps",exact:true})).toBeVisible();
  await expect(page.getByRole("button",{name:"Expand Blue Hours",exact:true})).toHaveCount(0);
  await expect(page.getByRole("checkbox",{name:/Select track/})).toHaveCount(0);
  await expect(page.getByRole("button",{name:"Expand Remote Favourite",exact:true})).toHaveCount(0);
  await page.getByRole("button",{name:"Actions for North Assembly Online",exact:true}).click();
  await page.getByRole("menuitem",{name:"View metadata / match details",exact:true}).click();
  await expect(page.getByRole("dialog")).toContainText("Local recordings");
  await expect(page.getByRole("dialog")).toContainText("North Assembly");
  expect(detailArtists).toEqual(["North Assembly"]);
  await page.getByRole("dialog").getByRole("button",{name:"Done",exact:true}).click();
  await expect(page.getByRole("alert")).toHaveCount(0);
  expect(errors).toEqual([]);
});
test("artist hierarchy keeps compilation releases together and resolves selected file paths at every level", async ({page}) => {
  const root=join(folder,"music"), requests:any[]=[];
  const args={route:"links",root,group_releases:true,group_artists:true,sort:"artist",direction:"asc",limit:1};
  const all=(await rpc("table",{...args,limit:100})).result;
  expect(all.total).toBe(3);
  expect(all.release_total).toBe(4);
  expect(all.track_total).toBe(7);
  const compilation=all.rows.find((row:any)=>row.artist==="Various Artists");
  expect(compilation.artist_group).toBe(true);
  expect(compilation.children).toHaveLength(1);
  expect(compilation.children[0].release).toBe("Night Sessions");
  expect(compilation.children[0].children.map((row:any)=>row.title)).toEqual(["Opening","Closing"]);
  const first=(await rpc("table",args)).result, second=(await rpc("table",{...args,offset:1})).result;
  expect(first.total).toBe(3);
  expect(first.rows).toHaveLength(1);
  expect(first.rows[0].id).not.toBe(second.rows[0].id);
  const north=all.rows.find((row:any)=>row.artist==="North Assembly");
  expect(north.children).toHaveLength(2);
  const paths=north.children.flatMap((release:any)=>release.children.map((track:any)=>track.path));
  await page.route("**/__test_rpc",async route=>{
    const request=route.request().postDataJSON();
    if(request.method==="job.start" && request.args.kind==="link") {
      requests.push(request);
      await route.fulfill({json:{result:{id:"artist-scope-check",kind:"link",status:"complete",message:"Selected files checked"}}});
    } else await route.fulfill({json:await rpc(request.method,request.args)});
  });
  await page.goto("/");
  await page.locator("aside").getByRole("button",{name:"Link releases",exact:true}).click();
  await columnAll(page,"Status");
  await expect(page.locator("tbody tr")).toHaveCount(3);
  await page.getByRole("checkbox",{name:"Select North Assembly",exact:true}).check();
  await page.getByRole("button",{name:"Recheck 4 selected",exact:true}).click();
  await expect.poll(()=>requests.length).toBe(1);
  expect(requests[0].args.args.ids.sort()).toEqual(paths.sort());
  await expandLocalArtist(page);
  await expect(page.getByRole("checkbox",{name:"Select Blue Hours",exact:true})).toBeChecked();
  await expect(page.getByRole("checkbox",{name:"Select Night Maps",exact:true})).toBeChecked();
  await page.getByRole("checkbox",{name:"Select Blue Hours",exact:true}).uncheck();
  await expect(page.getByRole("checkbox",{name:"Select North Assembly",exact:true})).toHaveJSProperty("indeterminate",true);
  const track=await localTrackRow(page,"Signal","Night Maps");
  await track.getByRole("checkbox",{name:"Select track Signal",exact:true}).uncheck();
  await expect(page.getByRole("checkbox",{name:"Select Night Maps",exact:true})).toHaveJSProperty("indeterminate",true);
  await page.getByRole("button",{name:"Release",exact:true}).click();
  await page.getByRole("button",{name:"Release",exact:true}).click();
  await expect(page.getByRole("button",{name:"Collapse North Assembly",exact:true})).toBeVisible();
  await expect(page.getByRole("button",{name:"Collapse Night Maps",exact:true})).toBeVisible();
  await page.getByRole("button",{name:"Recheck 1 selected",exact:true}).click();
  await expect.poll(()=>requests.length).toBe(2);
  expect(requests[1].args.args.ids).toEqual([join(root,"North Assembly/Night Maps (2022)/Disc 2/Afterimage.flac")]);
  await expandLocalArtist(page,"Various Artists");
  await expect(page.getByRole("button",{name:"Expand Night Sessions",exact:true})).toHaveCount(1);
  await page.getByRole("button",{name:"Expand Night Sessions",exact:true}).click();
  await expect(page.getByRole("checkbox",{name:"Select track Opening",exact:true})).toBeVisible();
  await expect(page.getByRole("checkbox",{name:"Select track Closing",exact:true})).toBeVisible();
  await expect(page.getByRole("alert")).toHaveCount(0);
});
test("online replacements group targets and preserve scoped selection, readable evidence and cached results", async ({page}) => {
  const root=join(folder,"music"), requests:any[]=[];
  const originalFiles=[join(root,"First Light.flac"),join(root,"Drift.flac"),join(root,"North Assembly/First Light (2019)/First Light.flac")];
  const originalBytes=originalFiles.map(path=>readFileSync(path));
  const all=(await rpc("table",{route:"online",root,limit:100,sort:"release",direction:"asc"})).result;
  expect(all.total).toBe(2);
  const deluxe=all.rows.find((row:any)=>row.release==="Blue Hours (Deluxe)");
  expect(deluxe.replacement_group).toBe(true);
  expect(deluxe.id).toBe("online-replacement:910003");
  expect(deluxe.children.map((row:any)=>row.release).sort()).toEqual(["Blue Hours","First Light"]);
  expect(deluxe.tracks).toBe(4);
  expect(deluxe.duplicates).toBe(3);
  expect(deluxe.gained).toBe(2);
  expect(deluxe.date).toBe("2023-04-03");
  const bluePlan=deluxe.children.find((row:any)=>row.release==="Blue Hours").id;
  const firstPlan=deluxe.children.find((row:any)=>row.release==="First Light").id;
  expect(bluePlan).toBe(`910003::${root}`);
  const filtered=(await rpc("table",{route:"online",root,column_filters:{release:{include:["Blue Hours (Deluxe)"]}},limit:100})).result;
  expect(filtered.total).toBe(1);
  expect(filtered.rows[0].children).toHaveLength(2);
  const childSearch=(await rpc("table",{route:"online",root,search:"First Light",limit:100})).result;
  expect(childSearch.total).toBe(1);
  expect(childSearch.rows[0].children.map((row:any)=>row.id).sort()).toEqual([bluePlan,firstPlan].sort());
  await rpc("queue.decision",{ids:["910001","910002","910003","910004","910005","910006"],decision:"removed"});
  await page.route("**/__test_rpc",async route=>{
    const request=route.request().postDataJSON();
    if(request.method==="job.start" && request.args.kind==="queue_replacements") requests.push(request);
    await route.fulfill({json:await rpc(request.method,request.args)});
  });
  await page.goto("/");
  const sidebar=page.locator("aside");
  await sidebar.getByRole("button",{name:"Online replacements",exact:true}).click();
  await expect(page.locator("tbody tr")).toHaveCount(2);
  await expect(page.getByRole("button",{name:"Filter Tracks",exact:true})).toHaveCount(0);
  await expect(page.getByRole("button",{name:"Filter Duplicate files",exact:true})).toHaveCount(0);
  await expect(page.getByRole("button",{name:"Filter Tracks gained",exact:true})).toHaveCount(0);
  await expect(page.getByRole("button",{name:"Filter Evidence",exact:true})).toHaveCount(0);
  const parent=page.getByRole("checkbox",{name:"Select Blue Hours (Deluxe)",exact:true});
  await parent.check();
  await page.getByRole("button",{name:"Expand Blue Hours (Deluxe)",exact:true}).click();
  const blue=page.getByRole("checkbox",{name:"Select local release Blue Hours",exact:true});
  const first=page.getByRole("checkbox",{name:"Select local release First Light",exact:true});
  await expect(blue).toBeChecked();
  await expect(first).toBeChecked();
  await page.locator("tbody tr").filter({has:blue}).click({button:"right"});
  await page.getByRole("menuitem",{name:"View metadata / match details",exact:true}).click();
  await expect(page.getByRole("dialog")).toContainText("Blue Hours");
  await page.getByRole("dialog").getByRole("button",{name:"Done",exact:true}).click();
  await expect(parent).toBeChecked();
  await expect(blue).toBeChecked();
  await expect(first).toBeChecked();
  await first.uncheck();
  await expect(parent).toHaveJSProperty("indeterminate",true);
  await expect(blue).toBeChecked();
  await page.getByRole("button",{name:"Release",exact:true}).click();
  await page.getByRole("button",{name:"Release",exact:true}).click();
  await expect(page.getByRole("button",{name:"Collapse Blue Hours (Deluxe)",exact:true})).toBeVisible();
  await expect(first).not.toBeChecked();
  await page.getByRole("button",{name:"Actions for Blue Hours (Deluxe)",exact:true}).click();
  await page.getByRole("menuitem",{name:"View metadata / match details",exact:true}).click();
  const dialog=page.getByRole("dialog");
  const affected=dialog.getByRole("table",{name:"Affected local releases",exact:true});
  await expect(affected).toContainText("Blue Hours");
  await expect(affected).toContainText("First Light");
  await expect(affected).toContainText("matching recording identifiers");
  await expect(dialog.locator("pre")).toHaveCount(0);
  await dialog.getByRole("button",{name:"Done",exact:true}).click();
  await expect(parent).toHaveJSProperty("indeterminate",true);
  await expect(first).not.toBeChecked();
  await expect(page.getByRole("button",{name:"Queue replacement releases (1)",exact:true})).toBeEnabled();
  await page.getByRole("button",{name:"Actions for Blue Hours (Deluxe)",exact:true}).click();
  await page.getByRole("menuitem",{name:"Queue selected replacement releases",exact:true}).click();
  await expect.poll(()=>requests.length).toBe(1);
  expect(requests[0].args.args.ids).toEqual([bluePlan]);
  await expect.poll(async()=>{
    const jobs=(await rpc("job.status")).result;
    return [jobs.job,jobs.online_job].find(job=>job?.kind==="queue_replacements")?.status;
  }).toBe("complete");
  const queue=(await rpc("table",{route:"queue",root,limit:100})).result;
  expect(queue.rows.map((row:any)=>row.id)).toEqual(["910003"]);
  for(let index=0;index<originalFiles.length;index++) expect(readFileSync(originalFiles[index])).toEqual(originalBytes[index]);
  await sidebar.getByRole("button",{name:"Local duplicates",exact:true}).click();
  await page.getByRole("button",{name:"Check local duplicates",exact:true}).click();
  await expect.poll(async()=>{
    const job=(await rpc("job.status")).result.job;
    return job?.kind==="local_duplicates" ? job.status : "waiting";
  }).toBe("complete");
  const retained=page.getByRole("checkbox",{name:"Select Blue Hours",exact:true});
  await retained.check();
  await page.getByRole("button",{name:"Expand Blue Hours",exact:true}).click();
  const duplicate=page.getByRole("checkbox",{name:"Select duplicate First Light",exact:true});
  await expect(duplicate).toBeChecked();
  await page.getByRole("button",{name:"Actions for First Light",exact:true}).click();
  await page.getByRole("menuitem",{name:"View metadata / match details",exact:true}).click();
  await expect(page.getByRole("dialog")).toContainText("First Light");
  await page.getByRole("dialog").getByRole("button",{name:"Done",exact:true}).click();
  await expect(retained).toBeChecked();
  await duplicate.uncheck();
  await expect(retained).not.toBeChecked();
  await expect(page.getByRole("button",{name:/^Review duplicate removal/})).toBeDisabled();
  await sidebar.getByRole("button",{name:"Overview",exact:true}).click();
  await sidebar.getByRole("button",{name:"Online replacements",exact:true}).click();
  await expect(page.getByRole("navigation",{name:"Table pagination",exact:true})).toContainText("1–2 of 2");
  await expect(page.getByRole("checkbox",{name:"Select Blue Hours (Deluxe)",exact:true})).toBeVisible();
  await expect(page.getByRole("alert")).toHaveCount(0);
  if(process.env.TIBRARY_SCREENSHOTS) {
    const expander=page.getByRole("button",{name:/^(Expand|Collapse) Blue Hours \(Deluxe\)$/});
    if(await expander.getAttribute("aria-expanded") !== "true") await expander.click();
    await page.screenshot({path:"/tmp/tibrary-online-replacements-0.9.20.png"});
  }
});
test("dark settings fit a full window and retain defaults", async ({
  page,
}) => {
  const saved = await rpc("settings.save", {
    section: "desktop",
    values: { market: "GB", theme: "dark" },
  });
  expect(saved.error).toBeUndefined();
  await page.goto("/");
  await page
    .locator("aside")
    .getByRole("button", { name: "General", exact: true })
    .click();
  await expect(
    page.getByRole("main", { name: "General", exact: true }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Reset to default" }).click();
  await expect(page.getByRole("alert")).toHaveCount(0);
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  await expect(
    page.getByRole("button", { name: "Reset to default" }),
  ).toBeEnabled();
});

test("release matching scope is saved and explained in settings", async ({page}) => {
  await page.goto("/");
  await page.locator("aside").getByRole("button", {name:"General", exact:true}).click();
  await expect(page.getByRole("heading", {name:"Release matching & recommendations"})).toBeVisible();
  const bootlegs = page.getByLabel("Include bootlegs and unofficial recordings");
  await expect(bootlegs).toHaveAttribute("type", "checkbox");
  await bootlegs.check();
  await expect.poll(async () => (await rpc("settings")).result?.general?.recommend_bootlegs).toBe(true);
  await expect(page.locator('label[title="Include bootlegs/unofficial live recordings in match candidates and artist recommendations"]')).toBeVisible();
});

test("missing releases queue only the selected audio tracks", async ({
  page,
}) => {
  await page.goto("/");
  await page
    .locator("aside")
    .getByRole("button", { name: "Missing releases", exact: true })
    .click();
  await viewOption(page, "Release timeline", "All missing releases");
  await page
    .getByRole("button", { name: "Expand Blue Hours", exact: true })
    .click();
  await expectTrackPositionInTracks(page, "First Light", "1 · 1");
  await page
    .getByRole("checkbox", { name: "Select Blue Hours", exact: true })
    .check();
  await page
    .getByRole("checkbox", { name: "Select track First Light", exact: true })
    .uncheck();
  await page
    .getByRole("button", { name: "Queue selected (1)", exact: true })
    .click();
  await expect(page.getByRole("alert")).toHaveCount(0);
  await page
    .locator("aside")
    .getByRole("button", { name: "Download queue", exact: true })
    .click();
  await expect(
    page.getByRole("checkbox", { name: "Select Blue Hours", exact: true }),
  ).toHaveJSProperty("indeterminate", true);
  await expect(
    page.getByRole("checkbox", {
      name: "Select track First Light",
      exact: true,
    }),
  ).not.toBeChecked();
});

test("window panes stay isolated and theme labels are readable", async ({
  page,
}) => {
  await page.goto("/");
  const nav = page.locator("aside");
  const names = await nav
    .locator(".nav-heading button:last-child")
    .allTextContents();
  expect(names.indexOf("Update library")).toBe(
    names.indexOf("Complete library") + 1,
  );
  await nav.getByRole("button", { name: "General", exact: true }).click();
  const theme = page.getByLabel("Colour theme");
  await expect(theme.locator("option")).toHaveText(["System", "Light", "Dark"]);
  const light = await page.evaluate(() => {
    document.documentElement.dataset.theme = "light";
    return getComputedStyle(document.body).backgroundColor;
  });
  const dark = await page.evaluate(() => {
    document.documentElement.dataset.theme = "dark";
    return getComputedStyle(document.body).backgroundColor;
  });
  expect(dark).not.toBe(light);
  await page.setViewportSize({ width: 1000, height: 680 });
  const drag = await page.locator(".drag-region").boundingBox();
  expect(drag?.width).toBe(1000);
  expect(drag?.y).toBe(0);
  await nav.evaluate((el) => {
    el.scrollTop = el.scrollHeight;
  });
  await page.locator(".scroll-page").evaluate((el) => {
    el.scrollTop = el.scrollHeight;
  });
  expect(await page.evaluate(() => window.scrollY)).toBe(0);
  expect((await page.locator(".drag-region").boundingBox())?.y).toBe(0);
  expect(
    await page
      .locator("main")
      .evaluate((el) => el.getBoundingClientRect().bottom),
  ).toBeLessThanOrEqual(680);
});

test("overview restores the library and shows cached missing releases", async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem("tibrary.root", "/old-library"));
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Latest missing releases" })).toBeVisible();
  await expect(page.getByLabel("Active library")).not.toHaveValue("/old-library");
  await expect(page.getByText("Choose a registered library first")).toHaveCount(0);
  await expect(page.getByRole("button", { name: "View missing releases" })).toBeVisible();
});

test("reviewed number corrections run through the UI and survive navigation", async ({ page }) => {
  await page.goto("/");
  await page.locator("aside").getByRole("button", { name: "Correct tags", exact: true }).click();
  await actionOption(page, "Choose correction", "Track & disc numbers", "menuitemradio");
  await page.getByRole("button", { name: "Preview changes", exact: true }).click();
  await expect(page.getByRole("button", { name: "Preview changes", exact: true })).toBeEnabled();
  await page.getByRole("checkbox", { name: "Select visible rows" }).check();
  await page.getByRole("button", { name: /Review & apply/ }).click();
  await expect(page.getByRole("dialog")).toBeVisible();
  await expect(page.getByRole("dialog")).toContainText("01");
  await page.getByRole("button", { name: "Confirm & continue" }).click();
  await expect.poll(async () => (await rpc("state")).result?.job?.status).toBe("complete");
  await expect(page.getByRole("alert")).toHaveCount(0);
  const meta=await rpc("detail",{path:join(folder,"music","First Light.flac")});
  expect(meta.result.tags.tracknumber).toEqual(["01"]);
});

test("account sign-in opens its prompt, rejects unrelated redirects and cancels", async ({ page }) => {
  await page.goto("/");
  await page.locator("aside").getByRole("button",{name:"General",exact:true}).click();
  await expect(page.getByRole("heading", {name:"Developer credentials",exact:true})).toHaveCount(0);
  await expect(page.getByLabel("Client secret")).toHaveCount(0);
  await page.getByRole("button",{name:"Connect account",exact:true}).click();
  await expect(page.getByRole("dialog")).toBeVisible();
  await expect(page.getByRole("button",{name:"Open sign-in page"})).toBeVisible();
  const invalid=await rpc("auth.reply",{response:"https://example.com/?code=wrong"});
  expect(invalid.error).toBeTruthy();
  expect((await rpc("state")).result.auth_url).toBeTruthy();
  await page.getByRole("dialog").getByRole("button",{name:"Cancel",exact:true}).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  expect((await rpc("state")).result.auth_url).toBeNull();
});

test("audit results survive navigation and unknown actions fail explicitly", async ({ page }) => {
  await page.goto("/");
  const started = await rpc("job.start", {kind: "mqa", args: {root: join(folder, "music")}});
  expect(started.error).toBeUndefined();
  await expect.poll(async () => (await rpc("job.status")).result?.job?.status).toBe("complete");
  const first = await rpc("table", {route: "mqa", root: join(folder, "music")});
  expect(first.result.total).toBe(2);
  await page.locator("aside").getByRole("button", {name: "Settings", exact: true}).click();
  await page.locator("aside").getByRole("button", {name: "MQA audit", exact: true}).click();
  const restored = await rpc("table", {route: "mqa", root: join(folder, "music")});
  expect(restored.result.rows).toEqual(first.result.rows);
  const status=first.result.rows[0].status;
  await columnOnly(page,"Status",[status]);
  await expect(page.locator("tbody tr")).toHaveCount(1);
  await expandLocalRelease(page);
  await expect(page.locator("tbody tr.child")).toHaveCount(first.result.rows.filter((row:any)=>row.status===status).length);
  expect((await rpc("job.start", {kind: "unknown_action"})).error).toBeTruthy();
});

test("download without an account fails and retains the approved queue", async () => {
  const queued = await rpc("table", {route: "queue"});
  expect(queued.result.total).toBeGreaterThan(0);
  await rpc("queue.select", {selection: {[queued.result.rows[0].id]: null}});
  const before = await rpc("table", {route: "queue"});
  expect(before.result.rows.some((r: any) => r.approved)).toBe(true);
  await rpc("job.start", {kind: "download"});
  await expect.poll(async () => (await rpc("job.status")).result?.download_job?.status).toBe("failed");
  const after = await rpc("table", {route: "queue"});
  expect(after.result.rows).toEqual(before.result.rows);
});

test("downloaded release and track menus requeue the original record", async ({page}) => {
  await rpc("queue.decision", {ids:["910001"],decision:"downloaded"});
  await page.goto("/");
  await page.locator("aside").getByRole("button", {name:"Downloaded releases",exact:true}).click();
  await page.getByRole("button", {name:"Actions for Blue Hours"}).click();
  await page.getByRole("menuitem", {name:"Redownload release"}).click();
  await expect.poll(async () => (await rpc("table", {route:"queue"})).result.rows.some((row:any) => row.id === "910001")).toBe(true);
  await rpc("queue.decision", {ids:["910001"],decision:"downloaded"});
  await page.reload();
  await page.locator("aside").getByRole("button", {name:"Downloaded releases",exact:true}).click();
  await page.getByRole("button", {name:"Expand Blue Hours"}).click();
  await page.getByRole("button", {name:"Actions for track Drift"}).click();
  await page.getByRole("menuitem", {name:"Redownload track"}).click();
  const queued = await rpc("table", {route:"queue"});
  expect(queued.result.rows.find((row:any) => row.id === "910001")?.selected).toEqual(["91000101"]);
});

test("activity has one verbose log and independent worker controls", async ({page}) => {
  await page.goto("/");
  await page.locator("aside").getByRole("button", {name:"Activity",exact:true}).click();
  const panel=page.getByRole("region", {name:"Activity log",exact:true});
  await expect(panel).toBeVisible();
  await expect(page.locator(".activity-status small")).toHaveText(["Local actions", "Online actions", "Downloads"]);
  await expect(page.locator(".activity-status h2")).toHaveText(["Awaiting task...", "Awaiting task...", "Awaiting task..."]);
  expect((await panel.boundingBox())?.height).toBeGreaterThan(500);
  await panel.getByRole("searchbox", {name:"Search activity"}).fill("nothing matches");
  await expect(panel.getByRole("searchbox")).toHaveValue("nothing matches");
  await panel.getByRole("button", {name:"Clear activity"}).click();
  await expect(page.getByRole("region", {name:"Downloads worker",exact:true})).toBeVisible();
  await expect(panel.getByRole("button",{name:/saved history|more details/})).toHaveCount(0);
});

test("activity action filters search saved history beyond the recent page and keep the log flat", async ({page}) => {
  const old = [
    {saved_id:1,at:"2026-09-30T12:00:00Z",message:"Adjusted first date",job_kind:"apply",category:"local"},
    {saved_id:2,at:"2026-09-30T12:00:01Z",message:"Adjusted second date",job_kind:"apply",category:"local"},
    {saved_id:3,at:"2026-09-30T12:00:02Z",message:"Reviewed changes",job_kind:"preview",category:"local"},
  ];
  const entries=[...old,...Array.from({length:502},(_,i)=>({saved_id:i+4,at:new Date(Date.parse("2026-10-05T12:00:00Z")+i*1000).toISOString(),message:`Updated artist ${i+1}`,job_kind:"discography",category:"online"}))];
  const live={at:"2026-10-05T14:00:00Z",message:"Checking local files",job_kind:"scan",category:"local"};
  const requests:any[]=[];
  await page.route("**/__test_rpc",async route=>{
    const request=route.request().postDataJSON();
    if (request.method === "logs.history") {
      requests.push(request.args);
      const candidates=entries.filter(entry=>(!request.args.before_id || entry.saved_id<request.args.before_id)
        && (!request.args.job_kinds || request.args.job_kinds.includes(entry.job_kind))
        && (!request.args.stream || entry.category===request.args.stream)
        && (!request.args.search || entry.message.includes(request.args.search))).slice().reverse();
      const pageEntries=candidates.slice(0,500);
      return route.fulfill({json:{result:{entries:pageEntries,next_before_id:candidates.length>500 ? pageEntries.at(-1)!.saved_id : null}}});
    }
    const response=await rpc(request.method,request.args);
    if (request.method === "state" && response.result) Object.assign(response.result,{job:null,online_job:null,download_job:null,logs:[live],download_monitor:{},activity_epochs:[0,0,0]});
    await route.fulfill({json:response});
  });
  await page.goto("/");
  await page.locator("aside").getByRole("button",{name:"Activity",exact:true}).click();
  const panel=page.getByRole("region",{name:"Activity log",exact:true});
  await expect(panel.locator(".log-row")).toHaveCount(501);
  await expect(panel).not.toContainText("Adjusted first date");
  const filter=panel.getByRole("combobox",{name:"Action type"});
  await filter.selectOption("apply");
  await expect(panel.locator(".log-row")).toHaveCount(2);
  await expect(panel).toContainText("Adjusted first date");
  expect(requests.at(-1).job_kinds).toEqual(["apply"]);
  await filter.selectOption("stream:local");
  await expect(panel.locator(".log-row")).toHaveCount(4);
  await expect(panel).toContainText("Checking local files");
  expect(requests.at(-1).stream).toBe("local");
  await panel.getByRole("searchbox").fill("Adjusted first date");
  await expect(panel.locator(".log-row")).toHaveCount(1);
  await expect.poll(()=>requests.at(-1).search).toBe("Adjusted first date");
  expect(requests.at(-1).stream).toBe("local");
  await expect(panel.locator(".batch-toggle")).toHaveCount(0);
  await expect(page.getByRole("region",{name:"Downloads worker",exact:true})).toBeVisible();
});

test("native macOS appearance follows theme contrast and focus while rapid settings preserve preferences", async ({page}) => {
  // Deliberately distinct values prove the native palette is used instead of
  // assuming fixed macOS colours or reusing the current appearance's colour.
  const palette={
    light:{accent:"#c94979",blue:"#236ecc",purple:"#ad49bc",pink:"#c94979",red:"#c84c44",orange:"#ed832b",yellow:"#d3ad1c",green:"#428d57",graphite:"#77818b",selection:"#f1bdd2",selection_text:"#24202a",table_header:"#f5f7f9"},
    dark:{accent:"#ec77ac",blue:"#4791ec",purple:"#c779d8",pink:"#ec77ac",red:"#ec746c",orange:"#f1a347",yellow:"#eed557",green:"#7bc18a",graphite:"#a4aeba",selection:"#684256",selection_text:"#f5f8ff",table_header:"#23272b"},
    light_high_contrast:{accent:"#a12b58",blue:"#164b9d",purple:"#7c2b88",pink:"#a12b58",red:"#98352e",orange:"#ad530d",yellow:"#86700e",green:"#285e37",graphite:"#46515f",selection:"#d7a0b8",selection_text:"#12101a",table_header:"#f7f9ff"},
    dark_high_contrast:{accent:"#ffb7d8",blue:"#9bc6ff",purple:"#ecb6fb",pink:"#ffb7d8",red:"#ffaaa4",orange:"#ffd095",yellow:"#ffed9c",green:"#b2efbd",graphite:"#d7e0ed",selection:"#6f315a",selection_text:"#ffffff",table_header:"#13171b"},
  };
  let name="Pink",hex="#ec77ac",reads=0;
  await rpc("settings.save",{section:"desktop",values:{highlight_colour:"orange",theme:"dark"}});
  await page.route("**/__test_rpc",async route => {
    const request = route.request().postDataJSON();
    if(request.method === "appearance.accent") {
      reads++;
      await route.fulfill({json:{result:{name,hex,palettes:palette}}});
    } else await route.fulfill({json:await rpc(request.method,request.args)});
  });
  await page.emulateMedia({colorScheme:"dark",contrast:"no-preference"});
  await page.goto("/");
  await page.locator("aside").getByRole("button", {name:"General",exact:true}).click();
  const theme=page.getByLabel("Colour theme");
  const pageSize=page.getByLabel("Items per page");
  const property=(key:string)=>page.locator("html").evaluate((element,key)=>getComputedStyle(element).getPropertyValue(key).trim(),key);
  const accent=()=>property("--accent");
  await expect(page.getByLabel("Highlight colour")).toHaveCount(0);
  expect(await page.locator("html").getAttribute("data-highlight")).toBeNull();
  // An old app-specific colour preference must not override macOS.
  await expect.poll(accent).toBe(palette.dark.accent);
  await expect(page.getByLabel("Catalogue market")).toHaveCount(0);
  for(const mode of ["light","dark"] as const) {
    await theme.selectOption(mode);
    const size=mode === "light" ? "100" : "250";
    // Back-to-back saves used to overwrite the theme with an older snapshot.
    await pageSize.selectOption(size);
    await expect(theme).toHaveValue(mode);
    await expect(pageSize).toHaveValue(size);
    await expect(page.locator("html")).toHaveAttribute("data-theme",mode);
    await expect.poll(async()=> (await rpc("settings")).result.general.theme).toBe(mode);
    await expect.poll(async()=> String((await rpc("settings")).result.general.page_size)).toBe(size);
    await expect.poll(accent).toBe(palette[mode].accent);
    await expect.poll(()=>property("--table-header")).toBe(palette[mode].table_header);
    await expect.poll(()=>property("--system-selection")).toBe(palette[mode].selection);
    await page.emulateMedia({contrast:"more"});
    await expect.poll(accent).toBe(palette[`${mode}_high_contrast`].accent);
    await expect.poll(()=>property("--table-header")).toBe(palette[`${mode}_high_contrast`].table_header);
    await page.emulateMedia({contrast:"no-preference"});
    await expect.poll(accent).toBe(palette[mode].accent);
  }
  // System appearance follows OS Light/Dark without replacing the saved choice.
  await theme.selectOption("system");
  for(const mode of ["light","dark"] as const) {
    await page.emulateMedia({colorScheme:mode});
    await expect.poll(accent).toBe(palette[mode].accent);
  }
  // Returning focus refreshes the native accent without an app preference.
  await theme.selectOption("dark");
  await expect.poll(accent).toBe(palette.dark.accent);
  palette.dark.accent="#62a4ee";
  name="Blue";hex=palette.dark.accent;
  const previousReads=reads;
  await page.evaluate(()=>window.dispatchEvent(new Event("focus")));
  await expect.poll(()=>reads).toBeGreaterThan(previousReads);
  await expect.poll(accent).toBe(palette.dark.accent);
  await expect(page.getByRole("alert")).toHaveCount(0);
});

test("startup always shows overview and does not replay a historical failure", async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem("tibrary.route", "local"));
  await page.route("**/__test_rpc", async route => {
    const request = route.request().postDataJSON();
    const response = await rpc(request.method, request.args);
    if (request.method === "state") response.result.job = {id:"old-failure",kind:"release_details",status:"failed",message:"Catalogue request failed (HTTP 400)",historical:true};
    await route.fulfill({json:response});
  });
  await page.goto("/");
  await expect(page.getByRole("main", {name:"Overview",exact:true})).toBeVisible();
  await expect(page.getByRole("alert")).toHaveCount(0);
  const missing = await rpc("table", {route:"missing",timeline:"All missing releases",status:"all",limit:20,artist_scope:"My album artists",recommendation:"Recommended and potential"});
  await expect(page.locator(".metric").filter({hasText:"Missing releases"})).toContainText(String(missing.result.total));
});

test("libraries and local tables load while dashboard statistics are still pending", async ({page}) => {
  await page.addInitScript(() => localStorage.removeItem("tibrary.root"));
  let releaseStats!:()=>void, reads=0;
  const pendingStats=new Promise<void>(resolve=>{releaseStats=resolve;});
  await page.route("**/__test_rpc",async route=>{
    const request=route.request().postDataJSON();
    const response=await rpc(request.method,request.args);
    if(request.method === "state" && !request.args.bootstrap) { reads++; await pendingStats; }
    await route.fulfill({json:response});
  });
  await page.goto("/");
  try {
    await expect(page.getByRole("combobox",{name:"Active library"})).toHaveValue(join(folder,"music"));
    await expect(page.getByRole("combobox",{name:"Active library"})).not.toContainText("Loading libraries");
    await expect(page.locator(".metric").filter({hasText:"Local tracks"})).toContainText("2 / 2");
    await expect(page.locator(".metric").filter({hasText:"Linked releases"}).locator("strong")).toHaveText("—");
    await page.locator(".metric").filter({hasText:"Local tracks"}).click();
    await expandLocalRelease(page);
    await expect(page.locator("tbody")).toContainText("First Light");
    expect(reads).toBe(1);
  } finally { releaseStats(); }
  await page.locator("aside").getByRole("button",{name:"Overview",exact:true}).click();
  await expect(page.locator(".metric").filter({hasText:"Linked releases"}).locator("strong")).toHaveText("1 / 1");
});

test("release details and download review show cached track names and exact approvals", async ({ page }) => {
  await page.goto("/");
  await page.locator("aside").getByRole("button", {name:"Download queue",exact:true}).click();
  await page.getByRole("checkbox", {name:"Select Blue Hours",exact:true}).check();
  await page.getByRole("button", {name:"Expand Blue Hours",exact:true}).dblclick();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await expect(page.getByRole("button", {name:"Collapse Blue Hours",exact:true})).toBeVisible();
  await page.getByRole("button", {name:"Collapse Blue Hours",exact:true}).press("Enter");
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await page.getByRole("button", {name:"Expand Blue Hours",exact:true}).press("Enter");
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await page.getByRole("checkbox", {name:"Select track First Light",exact:true}).uncheck();
  await page.getByRole("button", {name:"Download",exact:true}).click();
  const dialog = page.getByRole("dialog");
  await expect(dialog.getByRole("table", {name:"Tracks in Blue Hours"})).toBeVisible();
  await expect(dialog).toContainText("Drift");
  await expect(dialog).not.toContainText("First Light");
  await expect(dialog).toContainText("Release ID 910001");
  if (process.env.TIBRARY_SCREENSHOTS) await dialog.screenshot({path:"/tmp/tibrary-review-final.png"});
  await dialog.getByRole("button", {name:"Cancel",exact:true}).click();
  await page.locator("tbody tr").filter({has:page.getByRole("button",{name:"Collapse Blue Hours",exact:true})}).dblclick();
  await expect(page.getByRole("dialog")).toContainText("First Light");
  await expect(page.getByRole("dialog")).toContainText("Blue Hours");
  await page.getByRole("dialog").getByRole("button", {name:"Done",exact:true}).click();
  await page.getByRole("button",{name:"Actions for track Drift",exact:true}).click();
  await expect(page.getByRole("menuitem",{name:"Open on web",exact:true})).toBeVisible();
  await expect(page.getByRole("menuitem",{name:"Queue whole release",exact:true})).toHaveCount(0);
});

test("metadata and settings use readable views without implementation panels", async ({ page }) => {
  await page.goto("/");
  await page.locator("aside").getByRole("button", {name:"General",exact:true}).click();
  await expect(page.getByRole("heading",{name:"About Tibrary"})).toHaveCount(0);
  await page.locator("aside").getByRole("button", {name:"Link releases",exact:true}).click();
  await columnAll(page, "Status");
  await (await localTrackRow(page,"First Light")).dblclick();
  await page.getByText("All saved tags & DJ checks",{exact:true}).click();
  await expect(page.getByRole("table",{name:"Local file tags"})).toBeVisible();
  await expect(page.getByRole("dialog").locator("pre")).toHaveCount(0);
});

test("automatic correction previews can be applied and all-files view remains available", async ({page}) => {
  await page.goto("/");
  await page.locator("aside").getByRole("button",{name:"Correct tags",exact:true}).click();
  await actionOption(page, "Choose correction", "Track & disc numbers", "menuitemradio");
  await expect(page.getByRole("button",{name:"Preview changes",exact:true})).toHaveText("Preview track & disc numbers");
  await page.getByRole("button",{name:"Choose correction",exact:true}).click();
  const correctionMenu=page.getByRole("menu",{name:"Choose correction",exact:true});
  await expect(correctionMenu.getByRole("menuitemradio",{checked:true})).toHaveCount(1);
  await expect(correctionMenu.getByRole("menuitemradio",{name:"Track & disc numbers",exact:true})).toHaveAttribute("aria-checked","true");
  await correctionMenu.press("Escape");
  await expect(page.getByRole("button",{name:/Review & apply/})).toHaveCount(1);
  await expect(page.locator("tbody tr")).toHaveCount(1);
  await page.getByRole("checkbox",{name:"Select visible rows"}).check();
  await page.getByRole("button",{name:/Review & apply/}).click();
  await expect(page.getByRole("dialog").getByRole("table",{name:"Tag comparison"}).first()).toBeVisible();
  await page.getByRole("button",{name:"Confirm & continue"}).click();
  await expect.poll(async()=> (await rpc("job.status")).result?.job?.status).toBe("complete");
  await expect(page.getByRole("alert")).toHaveCount(0);
  await expect(page.locator("tbody tr")).toHaveCount(0);
  await expect(page.getByRole("checkbox", {name:"Affected files only",exact:true})).toHaveCount(0);
  await page.getByRole("button", {name:"Reset column filters",exact:true}).click();
  await expect(page.locator("tbody tr")).toHaveCount(1);
});

test("local actions group releases, retain scoped selection and apply only the reviewed real file", async ({page}) => {
  const root=join(folder,"music"), first=join(root,"First Light.flac"), second=join(root,"Drift.flac");
  const indexed=(await rpc("table",{route:"files",root,limit:100})).result.rows;
  expect(indexed).toHaveLength(4);
  const originalBytes=new Map<string,Buffer>(indexed.map((row:any)=>[row.id,readFileSync(row.id)]));
  const requests:any[]=[];
  await page.route("**/__test_rpc",async route=>{
    const request=route.request().postDataJSON();
    requests.push(request);
    await route.fulfill({json:await rpc(request.method,request.args)});
  });
  // A small page proves that releases, rather than individual files, are paged.
  await rpc("settings.save",{section:"desktop",values:{page_size:1}});
  await page.goto("/");
  const sidebar=page.locator("aside");
  await sidebar.getByRole("button",{name:"Correct tags",exact:true}).click();
  await actionOption(page,"Choose correction","Track & disc numbers","menuitemradio");
  await expect(page.locator("tbody tr")).toHaveCount(1);
  await expect(page.getByRole("navigation",{name:"Table pagination",exact:true})).toContainText("1–1 of 2 releases");
  const grouped=(await rpc("table",{route:"correct",root,action:"numbers",group_releases:true,limit:1,sort:"release",direction:"asc"})).result;
  expect(grouped.total).toBe(2);
  expect(grouped.release_total).toBe(2);
  expect(grouped.track_total).toBe(4);
  expect(grouped.rows[0].track_ids.slice().sort()).toEqual([first,second].sort());
  expect(grouped.rows[0].item).toBeUndefined();
  const flatPreview=(await rpc("preview",{id:grouped.preview_id})).result;
  expect(flatPreview.rows).toHaveLength(4);
  expect(flatPreview.rows.every((row:any)=>!row.file_group && row.id.endsWith(".flac"))).toBe(true);

  const parent=page.getByRole("checkbox",{name:"Select Blue Hours",exact:true});
  await parent.check();
  const firstRow=await localFileRow(page,"First Light");
  const secondCheck=page.getByRole("checkbox",{name:"Select track Drift",exact:true});
  await expect(firstRow.getByRole("checkbox")).toBeChecked();
  await expect(secondCheck).toBeChecked();
  await parent.uncheck();
  await expect(firstRow.getByRole("checkbox")).not.toBeChecked();
  await expect(secondCheck).not.toBeChecked();
  await parent.check();
  await expect(firstRow.getByRole("checkbox")).toBeChecked();
  await expect(secondCheck).toBeChecked();
  const trackColumn=page.getByRole("columnheader").filter({has:page.getByRole("button",{name:"Tracks",exact:true})});
  const [positionBox,tracksBox]=await Promise.all([firstRow.locator("td.track-position").boundingBox(),trackColumn.boundingBox()]);
  expect(positionBox!.x).toBeCloseTo(tracksBox!.x,0);
  await secondCheck.uncheck();
  await expect(parent).toHaveJSProperty("indeterminate",true);
  await expect(page.getByRole("button",{name:/Review & apply/})).toHaveText("Review & apply (1)");
  await page.getByRole("button",{name:"Next",exact:true}).click();
  await expect(page.getByRole("checkbox",{name:"Select Night Maps",exact:true})).toBeVisible();
  await page.getByRole("button",{name:"Previous",exact:true}).click();
  await expect(page.getByRole("button",{name:"Collapse Blue Hours",exact:true})).toBeVisible();
  await expect(secondCheck).not.toBeChecked();
  await columnOnly(page,"Release",["Blue Hours"]);
  await expect(page.getByRole("navigation",{name:"Table pagination",exact:true})).toContainText("of 1 releases");
  await page.getByRole("button",{name:"Release",exact:true}).click();
  await page.getByRole("button",{name:"Release",exact:true}).click();
  await expect(page.getByRole("button",{name:"Collapse Blue Hours",exact:true})).toBeVisible();
  await expect(parent).toHaveJSProperty("indeterminate",true);
  await page.getByRole("button",{name:"Release",exact:true}).click();
  if(process.env.TIBRARY_SCREENSHOTS) await page.screenshot({path:"/tmp/tibrary-correct-numbers-0.9.21.png"});

  await page.getByRole("button",{name:"Actions for Blue Hours",exact:true}).click();
  await page.getByRole("menuitem",{name:"View metadata / match details",exact:true}).click();
  const releaseReview=page.getByRole("dialog").filter({has:page.getByRole("heading",{name:"Release review",exact:true})});
  await expect(releaseReview.getByRole("table",{name:"Files in Blue Hours",exact:true}).locator("tbody tr")).toHaveCount(2);
  await expect(releaseReview).toContainText("First Light");
  await expect(releaseReview).toContainText("Drift");
  await releaseReview.getByRole("button",{name:"Done",exact:true}).click();
  await expect(parent).toHaveJSProperty("indeterminate",true);
  await page.getByRole("button",{name:/Review & apply/}).click();
  const review=page.getByRole("dialog").filter({has:page.getByRole("heading",{name:"Review changes",exact:true})});
  await expect(review.getByRole("table",{name:"Tag comparison",exact:true})).toHaveCount(1);
  await expect(review).toContainText("First Light.flac");
  await expect(review).not.toContainText("Drift.flac");
  await review.getByRole("button",{name:"Confirm & continue",exact:true}).click();
  await expect.poll(async()=>(await rpc("job.status")).result?.job?.status).toBe("complete");
  const applied=requests.find(request=>request.method==="job.start" && request.args.kind==="apply");
  expect(applied.args.args.ids).toEqual([first]);
  const refreshed=(await rpc("detail",{path:first})).result;
  expect(refreshed.tags.tracknumber).toEqual(["01"]);
  // Detail reads the indexed database metadata, independently of media reads.
  expect(refreshed.metadata.tags.tracknumber).toEqual(["01"]);
  expect(readFileSync(first).equals(originalBytes.get(first)!)).toBe(false);
  for(const [path,bytes] of originalBytes) if(path!==first) expect(readFileSync(path).equals(bytes)).toBe(true);

  await actionOption(page,"Choose correction","Dates","menuitemradio");
  await expect(page.getByRole("button",{name:/Review & apply/})).toBeDisabled();
  await page.getByRole("button",{name:"Reset column filters",exact:true}).click();
  await expect(page.getByRole("button",{name:"Collapse Blue Hours",exact:true})).toBeVisible();
  await expect(page.getByRole("checkbox",{name:"Select track First Light",exact:true})).not.toBeChecked();
  await actionOption(page,"Choose correction","Track & disc numbers","menuitemradio");
  await expect(page.locator("tbody tr.child")).toHaveCount(1);
  await expect(page.getByRole("checkbox",{name:"Select track Drift",exact:true})).toBeVisible();

  // Other local tables use the same cached hierarchy; navigation cannot start
  // a metadata download or mutate any of the disposable source files.
  const beforeNavigation=requests.filter(request=>request.method==="job.start").length;
  for(const [route,name] of [["organise","Organise files"],["metadata","Add missing tags"],["artwork","Fix artwork"],["mqa","MQA audit"]]) {
    await sidebar.getByRole("button",{name,exact:true}).click();
    await expandLocalRelease(page);
    const result=(await rpc("table",{route,root,group_releases:true,limit:100})).result;
    expect(result.total).toBe(2);
    expect(result.track_total).toBe(4);
    expect(result.rows.every((row:any)=>row.file_group && row.children.every((track:any)=>track.id.endsWith(".flac")))).toBe(true);
    await expect(page.getByRole("checkbox",{name:"Select track First Light",exact:true})).toBeVisible();
    await expect(page.getByRole("checkbox",{name:"Select track Drift",exact:true})).toBeVisible();
  }
  expect(requests.filter(request=>request.method==="job.start")).toHaveLength(beforeNavigation);
  expect(requests.filter(request=>request.method==="table" && ["correct","organise","metadata","artwork","mqa"].includes(request.args.route)).every(request=>request.args.group_releases===true)).toBe(true);
  await expect(page.getByRole("alert")).toHaveCount(0);
});

test("organise files previews and applies only to the disposable library", async ({page}) => {
  await page.goto("/");
  await page.locator("aside").getByRole("button",{name:"Organise files",exact:true}).click();
  await page.getByRole("button",{name:"Preview moves"}).click();
  await expect.poll(async () => (await rpc("job.status")).result?.job?.status).toBe("complete");
  await expect(page.locator("tbody tr").first()).toBeVisible();
  await (await localFileRow(page,"First Light")).dblclick();
  await expect(page.getByRole("dialog")).toContainText("Proposed folder layout");
  await expect(page.getByRole("dialog")).toContainText("Proposed path");
  if (process.env.TIBRARY_FOLDER_SCREENSHOT) await page.screenshot({path:"/tmp/tibrary-folder-preview.png"});
  await expect(page.getByRole("dialog").getByText("Available placements")).toHaveCount(0);
  await expect(page.getByRole("dialog").getByText("Proposed changes")).toHaveCount(0);
  await page.getByRole("button",{name:"Done",exact:true}).click();
  await page.getByRole("checkbox",{name:"Select visible rows"}).check();
  await page.getByRole("button",{name:/Review & apply/}).click();
  await expect(page.getByRole("dialog")).toBeVisible();
  await expect(page.getByRole("dialog")).toContainText("Current path");
  await expect(page.getByRole("dialog")).toContainText("Proposed path");
  await expect(page.getByRole("dialog").getByRole("columnheader",{name:"Tag",exact:true})).toHaveCount(0);
  await page.getByRole("button",{name:"Confirm & continue"}).click();
  await expect.poll(async () => (await rpc("job.status")).result?.job?.status).toBe("complete");
  const indexed = await rpc("table", {route:"files",root:join(folder,"music"),limit:20});
  expect(indexed.result.rows.every((row:any) => row.path.includes("/North Assembly/Blue Hours"))).toBe(true);
});

test("MQA scan scopes check the relevant files and selected scans preserve other cached rows", async ({page}) => {
  const root=join(folder,"music"), first=join(root,"First Light.flac"), second=join(root,"Drift.flac");
  const scans:any[]=[];
  await page.route("**/__test_rpc",async route=>{
    const request=route.request().postDataJSON();
    if(request.method==="job.start" && request.args.kind==="mqa") scans.push(request.args.args);
    await route.fulfill({json:await rpc(request.method,request.args)});
  });
  await page.goto("/");
  await page.locator("aside").getByRole("button",{name:"MQA audit",exact:true}).click();
  await expect(page.locator("tbody").getByText("Not audited").first()).toBeVisible();
  await expect(page.getByRole("checkbox",{name:"Affected files only",exact:true})).toHaveCount(0);
  const scan=page.getByRole("button",{name:"Scan selected tracks",exact:true});
  await expandLocalRelease(page);
  const firstCheck=page.getByRole("checkbox",{name:"Select track First Light",exact:true});
  const secondCheck=page.getByRole("checkbox",{name:"Select track Drift",exact:true});
  await expect(firstCheck).toBeChecked();
  await expect(secondCheck).toBeChecked();
  await expect(scan).toContainText("2");
  await secondCheck.uncheck();
  await expect(scan).toContainText("1");
  await scan.click();
  await expect.poll(async () => (await rpc("job.status")).result?.job?.status).toBe("complete");
  expect(scans[0].ids).toEqual([first]);
  expect(scans[0].force).toBe(true);
  expect((await rpc("job.status")).result.job.result.inspected).toBe(1);
  const inspected=(await rpc("table",{route:"mqa",root,limit:100})).result;
  expect(inspected.total).toBe(2);
  expect(inspected.rows.find((row:any)=>row.id===second).status).toBe("Not audited");
  expect(inspected.rows.find((row:any)=>row.id===first).status).not.toBe("Not audited");
  expect((await rpc("mqa.selection",{root,scope:"unscanned"})).result.ids).toEqual([second]);
  // Both scopes read the same audit cache; selecting a scope never scans audio.
  await page.locator("aside").getByRole("button",{name:"Overview",exact:true}).click();
  await page.locator("aside").getByRole("button",{name:"MQA audit",exact:true}).click();
  await expandLocalRelease(page);
  await expect(firstCheck).not.toBeChecked();
  await expect(secondCheck).toBeChecked();
  await page.getByRole("button",{name:"Choose scan scope",exact:true}).click();
  await page.getByRole("menuitemradio",{name:"All releases",exact:true}).click();
  await expect(firstCheck).toBeChecked();
  await expect(secondCheck).toBeChecked();
  await page.getByRole("button",{name:"Choose scan scope",exact:true}).click();
  await page.getByRole("menuitemradio",{name:"Unscanned releases only",exact:true}).click();
  await expect(firstCheck).not.toBeChecked();
  await expect(secondCheck).toBeChecked();
  expect(scans).toHaveLength(1);
  await page.getByRole("button",{name:"Choose scan scope",exact:true}).click();
  await page.getByRole("menuitemradio",{name:"All releases",exact:true}).press("Escape");
  await expect(page.getByRole("menuitemradio",{name:"All releases",exact:true})).toHaveCount(0);
  await expect(page.getByRole("button",{name:"Choose scan scope",exact:true})).toBeFocused();
  await page.getByRole("button",{name:"Clear selection",exact:true}).click();
  await expect(scan).toBeDisabled();
  await expect(page.getByRole("button",{name:"Find online matches",exact:true})).toBeDisabled();
  await expect(page.getByRole("button",{name:"Queue replacements",exact:true})).toBeDisabled();
  expect(scans).toHaveLength(1);
  await expect(page.getByRole("alert")).toHaveCount(0);
  await page.locator("aside").getByRole("button",{name:"Local duplicates",exact:true}).click();
  await page.getByRole("button",{name:"Check local duplicates",exact:true}).click();
  await expect.poll(async () => (await rpc("job.status")).result?.job?.status).toBe("complete");
  await expect(page.getByRole("button",{name:"Check local duplicates",exact:true})).toBeVisible();
  await expect(page.getByRole("main",{name:"Local duplicates",exact:true})).toBeVisible();
});

test("MQA replacement actions use selected signal tracks across pages and remain separate from scans", async ({page}) => {
  const rows=Array.from({length:55},(_,index)=>({
    id:`/disposable/track-${index}.flac`,path:`/disposable/track-${index}.flac`,artist:"North Assembly",
    release:`Release ${String(index).padStart(2,"0")}`,title:`Track ${index}`,
    status:[0,54].includes(index)?"MQA signal":"No signal found",
    evidence:[0,54].includes(index)?"Saved MQA encoder tag":"No MQA tags or audio signature",
    affected:[0,54].includes(index),target:[0,54].includes(index)?"Queue lossless replacement":"—",
  }));
  const starts:any[]=[];
  await page.route("**/__test_rpc",async route=>{
    const request=route.request().postDataJSON(), args=request.args||{};
    if(["table","table.facets"].includes(request.method) && args.route==="mqa") {
      if(request.method==="table.facets") {
        await route.fulfill({json:{result:mockFacets(rows,args)}});return;
      }
      const selected=filteredMockRows(rows,args).filter(row=>!args.search || `${row.artist} ${row.release} ${row.title}`.toLowerCase().includes(args.search.toLowerCase()));
      selected.sort((a:any,b:any)=>String(a[args.sort]||"").localeCompare(String(b[args.sort]||""))*(args.direction==="desc"?-1:1));
      const pageRows=selected.slice(args.offset||0,(args.offset||0)+(args.limit||50));
      await route.fulfill({json:{result:{rows:args.group_releases ? pageRows.map(row=>({...row,id:`local-file-release:${row.release}`,file_group:true,title:"1 file",tracks:1,track_ids:[row.id],children:[row]})) : pageRows,total:selected.length,track_total:selected.length}}});return;
    }
    if(request.method==="mqa.selection") {
      const ids:string[]=args.scope==="all" ? rows.map(row=>row.id) : args.scope==="unscanned" ? [] : args.ids||[];
      const detected=ids.filter(id=>rows.some(row=>row.id===id && row.affected));
      const unlinked=detected.filter(id=>id===rows[0].id), ready=detected.filter(id=>id===rows[54].id);
      await route.fulfill({json:{result:{ids,detected_ids:detected,unlinked_ids:unlinked,ready_ids:ready,counts:{selected:ids.length,detected:detected.length,unlinked:unlinked.length,ready:ready.length,queued:0}}}});return;
    }
    if(request.method==="job.start" && ["link","queue_mqa","mqa"].includes(args.kind)) {
      starts.push(args);
      const result=args.kind==="queue_mqa" ? {root:args.args.root,queued_paths:args.args.ids} : {};
      await route.fulfill({json:{result:{id:`safe-mqa-${starts.length}`,kind:args.kind,status:"complete",message:"Test action complete",started:Date.now()/1000,result}}});return;
    }
    await route.fulfill({json:await rpc(request.method,args)});
  });
  await page.goto("/");
  await page.locator("aside").getByRole("button",{name:"MQA audit",exact:true}).click();
  const scan=page.getByRole("button",{name:"Scan selected tracks",exact:true});
  const match=page.getByRole("button",{name:"Find online matches",exact:true});
  const queue=page.getByRole("button",{name:"Queue replacements",exact:true});
  await expect(scan).toBeDisabled();
  await expect(match).toBeDisabled();
  await expect(queue).toBeDisabled();
  await page.getByRole("textbox",{name:"Filter table",exact:true}).fill("Track 0");
  await columnOnly(page,"Status",["MQA signal"]);
  await page.getByRole("button",{name:"Choose scan scope",exact:true}).click();
  await page.getByRole("menuitemradio",{name:"All releases",exact:true}).click();
  await expect(page.getByRole("textbox",{name:"Filter table",exact:true})).toHaveValue("");
  await expect(page.getByRole("button",{name:"Filter Status",exact:true})).not.toHaveClass(/active/);
  await expect(page.locator("tbody tr")).toHaveCount(50);
  await expect(scan).toContainText("55");
  await expect(match).toContainText("1");
  await expect(queue).toContainText("1");
  // Track 54 is outside the rendered page, but its approved replacement is queued.
  await queue.click();
  await expect.poll(()=>starts.length).toBe(1);
  expect(starts[0].kind).toBe("queue_mqa");
  expect(starts[0].args.ids).toEqual([rows[54].id]);
  await expect(scan).toContainText("54");
  await expect(queue).toContainText("0");
  await expect(queue).toBeDisabled();
  await expandLocalRelease(page,"Release 00");
  const unlinked=page.getByRole("checkbox",{name:"Select track Track 0",exact:true});
  await expect(unlinked).toBeChecked();
  await unlinked.uncheck();
  await expect(match).toBeDisabled();
  await expect(queue).toBeDisabled();
  await unlinked.check();
  await expect(match).toBeEnabled();
  await match.click();
  await expect.poll(()=>starts.length).toBe(2);
  expect(starts[1].kind).toBe("link");
  expect(starts[1].args.ids).toEqual([rows[0].id]);
  await expect(page.locator(".mqa-actions")).toContainText("Scan");
  const tableBox=await page.locator(".table-scroll").boundingBox(), actionsBox=await page.locator(".mqa-actions").boundingBox();
  expect(actionsBox!.y).toBeGreaterThanOrEqual(tableBox!.y+tableBox!.height-1);
  await page.getByRole("button",{name:"Clear selection",exact:true}).click();
  await expect(scan).toBeDisabled();
  await expect(match).toBeDisabled();
  await expect(queue).toBeDisabled();
  expect(starts).toHaveLength(2);
  await expect(page.getByRole("alert")).toHaveCount(0);
});

test("local change checks reuse indexed tags and keep dependent controls gated across navigation", async ({page}) => {
  let holdCompletion=true;
  let running:any;
  const scans:any[]=[];
  await page.route("**/__test_rpc",async route=>{
    const request=route.request().postDataJSON();
    const response=await rpc(request.method,request.args);
    if(request.method==="job.start" && request.args.kind==="scan") {
      scans.push(request.args.args);
      running=response.result;
    }
    if(holdCompletion && running && ["job.status","state"].includes(request.method)) response.result.job=running;
    await route.fulfill({json:response});
  });
  await page.goto("/");
  await page.getByRole("button",{name:"Check local changes",exact:true}).click();
  await expect.poll(()=>scans.length).toBe(1);
  expect(scans[0].force).not.toBe(true);
  await page.locator("aside").getByRole("button",{name:"Link releases",exact:true}).click();
  await expect(page.getByRole("button",{name:"Linking options",exact:true})).toBeDisabled();
  await expect(page.getByRole("button",{name:"Link unresolved tracks",exact:true})).toBeDisabled();
  await page.locator("aside").getByRole("button",{name:"Missing releases",exact:true}).click();
  await expect(page.getByRole("button",{name:"Update missing releases",exact:true})).toBeDisabled();
  holdCompletion=false;
  await expect(page.getByRole("button",{name:"Update missing releases",exact:true})).toBeEnabled();
  const status=(await rpc("job.status")).result.job;
  expect(status.status).toBe("complete");
  expect(status.result.read).toBe(0);
  expect(status.result.unchanged).toBe(2);
  expect(scans).toHaveLength(1);
});

test("table reloads when saved page size arrives after the initial table", async ({page}) => {
  await rpc("settings.save",{section:"general",values:{market:"GB",page_size:25}});
  let releaseSettings: () => void = ()=>{};
  const held = new Promise<void>(resolve=>{releaseSettings=resolve});
  const limits: number[]=[];
  await page.route("**/__test_rpc",async route=>{
    const request=route.request().postDataJSON();
    if(request.method==="settings") await held;
    if(request.method==="table" && request.args.route==="queue") limits.push(request.args.limit);
    await route.fulfill({json:await rpc(request.method,request.args)});
  });
  await page.goto("/");
  await page.locator("aside").getByRole("button",{name:"Download queue",exact:true}).click();
  await expect.poll(()=>limits).toContain(50);
  releaseSettings();
  await expect.poll(()=>limits).toContain(25);
});


test("cached release recheck reports activity and preserves release order and totals", async ({page}) => {
  await page.goto("/");
  await page.locator(".metric").filter({hasText:"Missing releases"}).click();
  const args = {route:"missing",timeline:"All missing releases",sort:"date",direction:"desc",limit:100};
  const before = (await rpc("table",args)).result;
  expect(before.total).toBe(before.missing_total);
  await actionOption(page, "Release update options", "Recalculate saved results");
  await expect.poll(async () => (await rpc("job.status")).result.online_job?.status).toBe("complete");
  const after = (await rpc("table",args)).result;
  expect(after.rows.map((row:any)=>row.id)).toEqual(before.rows.map((row:any)=>row.id));
  expect(after.missing_total).toBe(before.missing_total);
  const state = (await rpc("state")).result;
  expect(state.logs.some((log:any)=>log.category === "online" && log.message.includes("Cached release check complete"))).toBe(true);
  expect(state.online_job.message).toContain("music files unchanged");
});


test("artist context menu opens legacy candidates and local recordings", async ({page}) => {
  await page.goto("/");
  await page.locator("aside").getByRole("button",{name:"Link artists",exact:true}).click();
  await page.locator("tbody tr").first().click({button:"right"});
  await page.getByRole("menuitem",{name:"View metadata / match details"}).click();
  await expect(page.getByRole("dialog")).toContainText("Local recordings");
  await page.getByRole("button",{name:"Add ID",exact:true}).first().click();
  await expect(page.getByRole("dialog").getByRole("textbox")).toHaveValue("900001");
  await expect(page.getByRole("button",{name:"Check these recording links"})).toBeEnabled();
});


test("match unresolved artists skips confirmed artists without network work", async ({page}) => {
  await page.goto("/");
  await page.locator("aside").getByRole("button",{name:"Link artists",exact:true}).click();
  await expect(page.getByRole("button",{name:"Refresh release list",exact:true})).toHaveCount(0);
  await expect(page.getByRole("button",{name:"Download track details",exact:true})).toHaveCount(0);
  await page.getByRole("button",{name:"Match artist",exact:true}).click();
  await expect.poll(async () => (await rpc("job.status")).result?.online_job?.status).toBe("complete");
  const status = await rpc("job.status");
  expect(status.result.online_job.result.checked).toBe(0);
});

test("missing release filters, bidirectional sort and paging preserve the release rows", async ({page}) => {
  await rpc("queue.decision",{ids:["910002","910003","910004","910005","910006"],decision:"removed"});
  const args={route:"missing",timeline:"All missing releases",artist_scope:"My album artists",sort:"release",direction:"asc",limit:2};
  const first=(await rpc("table",{...args,offset:0})).result;
  const second=(await rpc("table",{...args,offset:2})).result;
  expect(first.rows.length).toBe(2);
  expect(second.rows.length).toBeGreaterThan(0);
  expect(first.rows.map((r:any)=>r.id).some((id:string)=>second.rows.some((r:any)=>r.id===id))).toBe(false);
  const all=(await rpc("table",{...args,limit:100})).result;
  const reverse=(await rpc("table",{...args,limit:100,direction:"desc"})).result;
  expect(reverse.rows.map((r:any)=>r.id)).toEqual(all.rows.map((r:any)=>r.id).reverse());
  await page.goto("/");
  await page.locator("aside").getByRole("button",{name:"Missing releases",exact:true}).click();
  await columnOnly(page, "Type", ["EP"]);
  await expect(page.locator("tbody")).toContainText("Between Stations");
  await expect(page.locator("tbody")).not.toContainText("Night Maps");
  await columnAll(page, "Type");
  await page.getByRole("textbox",{name:"Filter table",exact:true}).fill("Night Maps");
  await expect(page.locator("tbody")).toContainText("Night Maps");
  await expect(page.locator("tbody")).not.toContainText("Between Stations");
});

test("missing release defaults keep low matches inspectable without inflating overview", async ({page}) => {
  const queries:any[]=[];
  const rows=[
    {id:"verified-release",artist:"North Assembly",release:"Verified catalogue",date:"2026-01-02",type:"SINGLE",tracks:1,status:"Missing release",recommendation:"Recommended"},
    {id:"potential-release",artist:"North Assembly",release:"Incomplete evidence",date:"2026-01-01",type:"SINGLE",tracks:1,status:"Missing release",recommendation:"Potential"},
    {id:"low-match-release",artist:"North Assembly",release:"Unrelated catalogue",date:"2026-01-01",type:"SINGLE",tracks:1,status:"Missing release",recommendation:"Suspect / Low match"},
    {id:"unmatched-release",artist:"North Assembly",release:"Unmatched catalogue",date:"2026-01-01",type:"SINGLE",tracks:1,status:"Missing release",recommendation:"Unmatched"},
    {id:"other-artist-release",artist:"Guest Artist",release:"Other artist catalogue",date:"2026-01-03",type:"SINGLE",tracks:1,status:"Missing release",recommendation:"Recommended",scope:"Other artist appearances"},
    {id:"unchecked-artist-release",artist:"Unknown Artist",release:"Unchecked artist catalogue",date:"2026-01-03",type:"SINGLE",tracks:1,status:"Missing release",recommendation:"Potential",scope:"Artist credits not checked"},
  ];
  await page.route("**/__test_rpc",async route=>{
    const request=route.request().postDataJSON();
    if(["table","table.facets"].includes(request.method) && request.args.route==="missing") {
      const scoped=rows.filter(row=>request.args.artist_scope!=="My album artists" || !row.scope);
      if (request.method === "table.facets") {
        await route.fulfill({json:{result:mockFacets(scoped,request.args)}});
        return;
      }
      queries.push(request.args);
      const confidence=request.args.recommendation==="Recommended and potential"
        ? scoped.filter(row=>["Recommended","Potential"].includes(row.recommendation))
        : scoped;
      const selected=filteredMockRows(confidence,request.args);
      await route.fulfill({json:{result:{rows:selected,total:selected.length,missing_total:selected.length}}});
      return;
    }
    await route.fulfill({json:await rpc(request.method,request.args)});
  });
  await page.goto("/");
  const latest=page.getByRole("region",{name:"Latest missing releases",exact:true});
  await expect(latest).toContainText("Verified catalogue");
  await expect(latest).toContainText("Incomplete evidence");
  await expect(latest).not.toContainText("Unrelated catalogue");
  await expect(latest).not.toContainText("Other artist catalogue");
  await expect(latest).not.toContainText("Unchecked artist catalogue");
  const metric=page.locator(".metric").filter({hasText:"Missing releases"});
  await expect(metric.locator("strong")).toHaveText("2");
  await expect(metric).toContainText("My album artists · Recommended and Potential");
  await metric.click();
  await expect(page.getByRole("combobox",{name:"Recommendation",exact:true})).toHaveCount(0);
  await expect(page.getByRole("combobox",{name:"Table filter",exact:true})).toHaveCount(0);
  await expect(page.getByRole("combobox",{name:"Copyright match",exact:true})).toHaveCount(0);
  await page.getByRole("button",{name:"View options",exact:true}).click();
  const options=page.getByRole("dialog",{name:"Table view options",exact:true});
  await expect(options.getByRole("radio",{name:"My album artists",exact:true})).toBeChecked();
  await expect(options.getByRole("radio",{name:"All missing releases",exact:true})).toBeChecked();
  await options.getByRole("button",{name:"Close view options",exact:true}).click();
  const confidenceMenu=await columnMenu(page,"Recommendation");
  await expect(columnValue(confidenceMenu,"Recommended")).toHaveAttribute("aria-checked","true");
  await expect(columnValue(confidenceMenu,"Potential")).toHaveAttribute("aria-checked","true");
  await expect(columnValue(confidenceMenu,"Suspect / Low match")).toHaveAttribute("aria-checked","false");
  await confidenceMenu.press("Escape");
  await expect(page.locator("tbody")).toContainText("Verified catalogue");
  await expect(page.locator("tbody")).not.toContainText("Unrelated catalogue");
  await expect(page.getByRole("navigation",{name:"Table pagination",exact:true})).toContainText("1–2 of 2");
  await columnAll(page,"Recommendation");
  await expect(page.locator("tbody")).toContainText("Unrelated catalogue");
  await expect(page.locator("tbody")).toContainText("Unmatched catalogue");
  await columnOnly(page,"Recommendation",["Suspect / Low match"]);
  await expect(page.locator("tbody")).toContainText("Unrelated catalogue");
  await expect(page.locator("tbody")).not.toContainText("Verified catalogue");
  expect(queries[0].recommendation).toBe("Recommended and potential");
  expect(queries[0].artist_scope).toBe("My album artists");
  expect(queries.every(args=>!("copyright" in args))).toBe(true);
  await page.locator("aside").getByRole("button",{name:"Overview",exact:true}).click();
  await expect(metric.locator("strong")).toHaveText("2");
});

test("column menus filter, sort and preserve table interaction state", async ({page}) => {
  await rpc("settings.save",{section:"general",values:{page_size:100}});
  const rows=Array.from({length:60},(_,index)=>({id:`header-release-${index}`,artist:"North Assembly",release:`Release ${String(index).padStart(2,"0")}`,date:"2026-01-01",type:index%2 ? "EP" : "ALBUM",tracks:1,status:"Missing release",recommendation:"Potential",expanded_available:true,children:[{id:`header-track-${index}`,title:`Track ${index}`,position:"01",duration:180}]}));
  const queries:any[]=[];
  await page.route("**/__test_rpc",async route=>{
    const request=route.request().postDataJSON();
    if(["table","table.facets"].includes(request.method) && request.args.route==="missing") {
      if (request.method === "table.facets") {
        await route.fulfill({json:{result:mockFacets(rows,request.args)}});
        return;
      }
      queries.push(request.args);
      const selected=filteredMockRows(rows,request.args);
      selected.sort((a:any,b:any)=>String(a[request.args.sort]||"").localeCompare(String(b[request.args.sort]||""))*(request.args.direction==="desc" ? -1 : 1));
      await route.fulfill({json:{result:{rows:selected,total:selected.length,missing_total:selected.length}}});
      return;
    }
    await route.fulfill({json:await rpc(request.method,request.args)});
  });
  await page.goto("/");
  await page.locator("aside").getByRole("button",{name:"Missing releases",exact:true}).click();
  const selected=page.getByRole("checkbox",{name:"Select Release 00",exact:true});
  await expect(page.getByRole("button",{name:"Filter Album artist",exact:true})).not.toHaveClass(/active/);
  await expect(page.getByRole("button",{name:"Filter Recommendation",exact:true})).toHaveClass(/active/);
  await selected.check();
  await page.getByRole("button",{name:"Expand Release 00",exact:true}).click();
  await expect(page.getByRole("checkbox",{name:"Select track Track 0",exact:true})).toBeChecked();
  const scroller=page.locator(".table-scroll");
  await scroller.evaluate(element=>{element.scrollTop=180;});
  const scroll=await scroller.evaluate(element=>element.scrollTop);
  expect(scroll).toBeGreaterThan(0);
  const releaseHeader=page.locator("th").filter({has:page.getByRole("button",{name:"Release",exact:true})});
  const releaseSort=releaseHeader.getByRole("button",{name:"Release",exact:true});
  await releaseSort.focus();
  await releaseSort.press("Shift+F10");
  const menu=page.getByRole("menu",{name:"Release column options"});
  await expect(menu).toBeVisible();
  await menu.press("ArrowDown");
  await expect(menu.getByRole("menuitemradio",{name:"Sort descending",exact:true})).toBeFocused();
  await menu.press("Escape");
  await expect(releaseSort).toBeFocused();
  expect(await scroller.evaluate(element=>element.scrollTop)).toBe(scroll);
  await releaseHeader.click({button:"right"});
  await menu.getByRole("menuitemradio",{name:"Sort descending",exact:true}).click();
  await expect(releaseHeader).toHaveAttribute("aria-sort","descending");
  await expect(page.locator("tbody tr").first()).toContainText("Release 59");
  await expect(page.getByRole("button",{name:"Collapse Release 00",exact:true})).toHaveCount(1);
  await expect(selected).toBeChecked();
  const count=queries.length;
  await releaseHeader.click({button:"right"});
  await expect(menu.getByRole("menuitemradio",{name:"Sort descending",exact:true})).toHaveAttribute("aria-checked","true");
  await menu.getByRole("menuitemradio",{name:"Sort descending",exact:true}).click();
  expect(queries.length).toBe(count);
  await page.getByRole("button",{name:"Filter Recommendation",exact:true}).click();
  const recommendationMenu=page.getByRole("menu",{name:"Recommendation column options"});
  const bounds=(await recommendationMenu.boundingBox())!;
  expect(bounds.x+bounds.width).toBeLessThanOrEqual(1600);
  await expect(columnValue(recommendationMenu,"Potential")).toHaveAttribute("aria-checked","true");
  await recommendationMenu.getByRole("menuitem",{name:"Clear selection",exact:true}).click();
  await expect(columnValue(recommendationMenu,"Potential")).toHaveAttribute("aria-checked","false");
  await expect(page.locator("tbody tr")).toHaveCount(0);
  await columnValue(recommendationMenu,"Potential").click();
  await expect(recommendationMenu).toBeVisible();
  await recommendationMenu.press("Escape");
  await columnOnly(page,"Type",["ALBUM"]);
  await expect(page.getByRole("button",{name:"Filter Type",exact:true})).toHaveClass(/active/);
  await expect(page.getByRole("navigation",{name:"Table pagination",exact:true})).toContainText("1–30 of 30");
  await expect(selected).toBeChecked();
  await expect(page.getByRole("button",{name:"Collapse Release 00",exact:true})).toHaveCount(1);
  const typeMenu=await columnMenu(page,"Type");
  await expect(columnValue(typeMenu,"ALBUM")).toHaveAttribute("aria-checked","true");
  await expect(columnValue(typeMenu,"EP")).toHaveAttribute("aria-checked","false");
  if(process.env.TIBRARY_SCREENSHOTS) await page.screenshot({path:"/tmp/tibrary-column-checkbox-filters.png"});
  await columnValue(typeMenu,"EP").click();
  await expect(typeMenu).toBeVisible();
  await expect(page.getByRole("navigation",{name:"Table pagination",exact:true})).toContainText("1–60 of 60");
  await columnValue(typeMenu,"ALBUM").click();
  await expect(page.getByRole("navigation",{name:"Table pagination",exact:true})).toContainText("1–30 of 30");
  await typeMenu.getByRole("menuitem",{name:"Select all",exact:true}).click();
  await typeMenu.press("Escape");
  await expect(page.getByRole("button",{name:"Filter Type",exact:true})).not.toHaveClass(/active/);
  expect(queries.some(args=>args.column_filters?.type?.include?.join()==="ALBUM" && args.column_filters?.recommendation?.include?.join()==="Potential")).toBe(true);
  await page.locator("aside").getByRole("button",{name:"Link releases",exact:true}).click();
  await columnOnly(page,"Status",["Linked"]);
  await expect(page.locator("tbody tr")).toHaveCount(1);
  await expect(page.getByRole("alert")).toHaveCount(0);
});

test("real table facets filter before paging and share checkbox rules across workflows", async ({page}) => {
  const root=join(folder,"music");
  const args={route:"queue",root,filter:"all",sort:"release",direction:"asc",limit:1};
  const first=(await rpc("table",args)).result;
  expect(first.rows).toHaveLength(1);
  expect(first.total).toBe(6);
  const facets=(await rpc("table.facets",{...args,column:"release",limit:100})).result;
  expect(facets.total).toBe(6);
  expect(facets.options.map((option:any)=>option.value)).toContain("Night Maps");
  const chosen={release:{include:["Night Maps","Distant Rooms"]}};
  const filtered=(await rpc("table",{...args,column_filters:chosen})).result;
  expect(filtered.total).toBe(2);
  const second=(await rpc("table",{...args,column_filters:chosen,offset:1})).result;
  expect([filtered.rows[0].release,second.rows[0].release].sort()).toEqual(["Distant Rooms","Night Maps"]);
  const selfFacet=(await rpc("table.facets",{...args,column:"release",column_filters:chosen,limit:100})).result;
  expect(selfFacet.total).toBe(6);
  const none=(await rpc("table",{...args,column_filters:{...chosen,type:{include:["EP"]}}})).result;
  expect(none.total).toBe(0);
  const exclusion=(await rpc("table",{...args,column_filters:{release:{exclude:["Blue Hours"]}}})).result;
  expect(exclusion.total).toBe(5);
  const artists=(await rpc("table.facets",{route:"artists",root,column:"status"})).result;
  expect(artists.options.map((option:any)=>option.value)).toEqual(["Confirmed"]);
  const links=(await rpc("table.facets",{route:"links",root,column:"status"})).result;
  expect(links.options.map((option:any)=>option.value)).toEqual(["Linked"]);
  await page.goto("/");
  const sidebar=page.locator("aside");
  await sidebar.getByRole("button",{name:"Download queue",exact:true}).click();
  await columnOnly(page,"Release",["Distant Rooms","Night Maps"]);
  await expect(page.locator("tbody tr")).toHaveCount(2);
  await page.getByRole("checkbox",{name:"Select Night Maps",exact:true}).check();
  await page.getByRole("button",{name:"Expand Night Maps",exact:true}).click();
  await expect(page.getByRole("checkbox",{name:"Select track Signal",exact:true})).toBeChecked();
  await columnOnly(page,"Type",["ALBUM"]);
  await expect(page.getByRole("checkbox",{name:"Select Night Maps",exact:true})).toBeChecked();
  await expect(page.getByRole("button",{name:"Collapse Night Maps",exact:true})).toBeVisible();
  await sidebar.getByRole("button",{name:"Link artists",exact:true}).click();
  await columnOnly(page,"Match status",["Confirmed"]);
  await expect(page.locator("tbody tr")).toHaveCount(1);
  await sidebar.getByRole("button",{name:"Link releases",exact:true}).click();
  await columnOnly(page,"Status",["Linked"]);
  await expect(page.locator("tbody tr")).toHaveCount(1);
  await sidebar.getByRole("button",{name:"Correct tags",exact:true}).click();
  await actionOption(page, "Choose correction", "Track & disc numbers", "menuitemradio");
  await expect(page.locator("tbody tr")).toHaveCount(1);
  const tagMenu=await columnMenu(page,"Proposed tag changes");
  await expect(tagMenu.getByRole("menuitemcheckbox").first()).toBeVisible();
  expect(await tagMenu.getByRole("menuitemcheckbox").count()).toBeGreaterThan(0);
  await tagMenu.press("Escape");
  const artistMenu=await columnMenu(page,"Album artist");
  await expect(columnValue(artistMenu,"North Assembly")).toHaveAttribute("aria-checked","true");
  await columnValue(artistMenu,"North Assembly").click();
  await expect(page.locator("tbody tr")).toHaveCount(0);
  await expect(columnValue(artistMenu,"North Assembly")).toHaveAttribute("aria-checked","false");
  await columnValue(artistMenu,"North Assembly").click();
  await artistMenu.press("Escape");
  await expect(page.locator("tbody tr")).toHaveCount(1);
  await expect(page.getByRole("combobox",{name:"Table filter",exact:true})).toHaveCount(0);
  await expect(page.getByRole("alert")).toHaveCount(0);
});

test("missing release metadata shows saved credits and independent confidence filters", async ({page}) => {
  await rpc("queue.decision",{ids:["910001","910002","910003","910004","910005","910006"],decision:"removed"});
  const queries:any[]=[];
  const updates:any[]=[];
  await page.route("**/__test_rpc",async route=>{
    const request=route.request().postDataJSON();
    if(request.method==="job.start") {
      updates.push(request.args);
      await route.fulfill({json:{result:{id:"metadata-update",kind:request.args.kind,status:"complete",message:"Saved metadata reused"}}});
      return;
    }
    const response=await rpc(request.method,request.args);
    if(request.method==="table" && request.args.route==="missing") queries.push(request.args);
    if(request.method==="detail" && request.args.release_id && response.result) {
      response.result={...response.result,track_count:1,tracks_loaded:true,
        catalogue_metadata_status:{status:"checked",fields:{genres:"not_supplied",label:"not_supplied",providers:"supplied",replacement:"not_supplied"}},
        providers:[{id:"provider-1",name:"Example distributor"}],
        tracks:[{id:"recording-1",title:"Saved recording",disc_number:1,track_number:1,isrc:"GBTEST0000001",credits_complete:true,
          credits:[{role:"Composer",name:"Reference writer",contributor_id:42}],artists:[{id:"900001",name:"North Assembly",type:"MAIN"}]}]};
    }
    await route.fulfill({json:response});
  });
  await page.goto("/");
  await page.locator("aside").getByRole("button",{name:"Missing releases",exact:true}).click();
  await viewOption(page, "Release timeline", "All missing releases");
  await columnOnly(page,"Recommendation",["Potential"]);
  await expect.poll(()=>queries.some(args=>args.artist_scope==="My album artists" && args.column_filters?.recommendation?.include?.join()==="Potential")).toBe(true);
  await columnAll(page,"Recommendation");
  const row=page.locator("tbody tr").filter({hasText:"Night Maps"}).first();
  await expect(row).toBeVisible();
  await row.click({button:"right"});
  await page.getByRole("menuitem",{name:"View metadata / match details",exact:true}).click();
  const dialog=page.getByRole("dialog");
  await expect(dialog).toContainText("Track details and credits checked · 1/1 tracks");
  await expect(dialog.getByRole("table",{name:"Catalogue checks"})).toContainText("Checked · not supplied by the service");
  await expect(dialog).toContainText("Example distributor");
  await dialog.locator("summary").filter({hasText:"Saved recording"}).click();
  await expect(dialog.getByRole("table",{name:"Credits for Saved recording"})).toContainText("Reference writer");
  await expect(dialog).toContainText("Performer credits: North Assembly (main)");
  await dialog.getByRole("button",{name:"Get missing metadata",exact:true}).click();
  await expect.poll(()=>updates.length).toBe(1);
  expect(updates[0].kind).toBe("release_details");
  expect(updates[0].args.force).toBeUndefined();
  await expect(dialog.getByRole("button",{name:"Get missing metadata",exact:true})).toBeEnabled();
  await page.unrouteAll({behavior:"wait"});
});

test("catalogue refresh publishes partial results without resetting table state or restarting work", async ({page}) => {
  await rpc("queue.decision",{ids:["910001","910002","910003","910004","910005","910006"],decision:"removed"});
  await page.addInitScript(() => {
    let id=0;
    const callbacks=new Map<number,(event:any)=>void>();
    const listeners=new Map<number,string>();
    (window as any).__TAURI_INTERNALS__={
      transformCallback:(callback:(event:any)=>void)=>{callbacks.set(++id,callback);return id;},
      invoke:async(command:string,args:any)=>{
        if(command==="plugin:event|listen") {listeners.set(args.handler,args.event);return args.handler;}
        if(command==="plugin:event|unlisten") callbacks.delete(args.eventId);
      },
    };
    (window as any).__TAURI_EVENT_PLUGIN_INTERNALS__={unregisterListener:(_event:string,eventId:number)=>listeners.delete(eventId)};
    (window as any).emitBackendEvent=(payload:any)=>{
      for(const [handler,event] of listeners) if(event==="backend-event") callbacks.get(handler)?.({event,id:handler,payload});
    };
  });
  const job={id:"refresh-in-progress",kind:"discography",status:"running",message:"Checking North Assembly releases",started:Date.now()/1000,completed:1,total:20};
  let phase=0, stateCalls=0, starts=0;
  await page.route("**/__test_rpc",async route=>{
    const request=route.request().postDataJSON();
    if(request.method==="job.start") starts++;
    const response=await rpc(request.method,request.args);
    if(["state","job.status"].includes(request.method) && response.result) {
      response.result.online_job=job;
      if(request.method==="state") {stateCalls++;response.result.revision=500+phase;}
      else delete response.result.revision;
    }
    if(request.method==="table" && request.args.route==="missing") {
      const rows=response.result.rows;
      const base=rows.find((row:any)=>row.release==="Blue Hours");
      response.result.rows=[...rows.filter((row:any)=>!phase || row.release!=="Night Maps"),
        ...Array.from({length:30},(_,index)=>({...base,id:`cached-${index}`,release:`Cached release ${index+1}`,children:[],expanded_available:true}))];
      response.result.total=response.result.rows.length;
    }
    await route.fulfill({json:response});
  });
  await page.goto("/");
  await page.locator("aside").getByRole("button",{name:"Missing releases",exact:true}).click();
  await viewOption(page, "Release timeline", "All missing releases");
  await page.getByRole("button",{name:"Coverage",exact:true}).click();
  await page.getByRole("button",{name:"Coverage",exact:true}).click();
  await expect(page.locator("th[aria-sort='descending']")).toContainText("Coverage");
  await expect(page.locator("tbody")).toContainText("Night Maps");
  await page.getByRole("button",{name:"Expand Blue Hours",exact:true}).click();
  const childRow=page.locator("tr").filter({has:page.getByRole("checkbox",{name:"Select track First Light",exact:true})});
  await expect(childRow).toBeVisible();
  await childRow.evaluate(element=>element.setAttribute("data-retained","yes"));
  const scroller=page.locator(".table-scroll");
  await scroller.evaluate(element=>{element.scrollTop=80;});
  const scrollTop=await scroller.evaluate(element=>element.scrollTop);
  expect(scrollTop).toBeGreaterThan(0);
  const before=stateCalls;
  phase=1;
  await page.evaluate(()=>{
    for(let revision=501;revision<=503;revision++) (window as any).emitBackendEvent({event:"changed",reason:"catalogue-refresh",revision});
  });
  await expect(page.locator("tbody")).not.toContainText("Night Maps");
  expect(stateCalls-before).toBe(1);
  await expect(page.locator("th[aria-sort='descending']")).toContainText("Coverage");
  await expect(page.getByRole("button",{name:"Collapse Blue Hours",exact:true})).toHaveAttribute("aria-expanded","true");
  await expect(childRow).toHaveAttribute("data-retained","yes");
  expect(await scroller.evaluate(element=>element.scrollTop)).toBe(scrollTop);
  await expect(page.getByRole("button",{name:"Update missing releases",exact:true})).toBeDisabled();
  expect(starts).toBe(0);
  await expect(page.getByRole("alert")).toHaveCount(0);
});

test("missing release availability checks use the requested scope and preserve unavailable inspection", async ({page}) => {
  await rpc("queue.decision",{ids:["910001","910002","910003","910004","910005","910006"],decision:"removed"});
  const checks:any[]=[];
  let job:any;
  await page.route("**/__test_rpc", async route => {
    const request=route.request().postDataJSON();
    if (request.method==="job.start" && request.args.kind==="check_availability") {
      checks.push(request.args.args);
      job={id:`availability-${checks.length}`,kind:"check_availability",status:checks.length===1?"running":"complete",
        message:"Checking release availability · North Assembly — Blue Hours · GB",started:Date.now()/1000,completed:0,total:1};
      await route.fulfill({json:{result:job}});
      return;
    }
    const response=await rpc(request.method,request.args);
    if(job && ["state","job.status"].includes(request.method) && response.result) response.result.online_job=job;
    await route.fulfill({json:response});
  });
  await page.goto("/");
  const sidebar=page.locator("aside");
  await sidebar.getByRole("button",{name:"Missing releases",exact:true}).click();
  await viewOption(page, "Release timeline", "All missing releases");
  await expect(page.locator("tbody")).not.toContainText("Private Weather");
  await columnAll(page,"Recommendation");
  await viewOption(page,"Album artist scope","All artist appearances");
  await columnOnly(page,"Coverage",["Unavailable"]);
  await expect(page.locator("tbody")).toContainText("Private Weather");
  await expect(page.getByRole("button",{name:"View options",exact:true})).toHaveAttribute("title",/All missing releases/);
  await columnAll(page, "Coverage");
  await viewOption(page,"Album artist scope","My album artists");
  await columnOnly(page,"Coverage",["Missing release","Owned partial"]);
  await page.getByRole("checkbox",{name:"Select Blue Hours",exact:true}).check();
  await actionOption(page, "Release update options", "Check availability");
  await expect.poll(()=>checks.length).toBe(1);
  expect(checks[0].ids).toEqual(["910001"]);
  expect(checks[0].force).toBeUndefined();
  await expect(page.getByRole("button",{name:"Release update options",exact:true})).toBeDisabled();
  await sidebar.getByRole("button",{name:"Overview",exact:true}).click();
  await expect(page.getByRole("button",{name:"Show activity: Check release availability",exact:true})).toBeVisible();
  job={...job,status:"complete",completed:1,message:"Availability check complete · cached results saved"};
  await sidebar.getByRole("button",{name:"Missing releases",exact:true}).click();
  await expect(page.getByRole("button",{name:"Release update options",exact:true})).toBeEnabled();
  const visible=(await rpc("table",{route:"missing",timeline:"All missing releases",recommendation:"My album artists",filter:"all",limit:50})).result.rows;
  await actionOption(page, "Release update options", "Check availability");
  await expect.poll(()=>checks.length).toBe(2);
  expect([...checks[1].ids].sort()).toEqual(visible.map((row:any)=>row.id).sort());
  await page.getByRole("button",{name:"Expand Blue Hours",exact:true}).click();
  await page.getByRole("button",{name:"Actions for track First Light",exact:true}).click();
  await page.getByRole("menuitem",{name:"Check release availability",exact:true}).click();
  await expect.poll(()=>checks.length).toBe(3);
  expect(checks[2].ids).toEqual(["910001"]);
  expect(checks[2].force).toBe(true);
  await actionOption(page, "Release update options", "Check saved release availability");
  await expect.poll(()=>checks.length).toBe(4);
  expect(checks[3].ids).toBeUndefined();
  expect(checks[3].force).toBeUndefined();
  await actionOption(page, "Release update options", "Recheck saved availability online");
  await expect.poll(()=>checks.length).toBe(5);
  expect(checks[4].ids).toBeUndefined();
  expect(checks[4].force).toBe(true);
  await expect(page.getByRole("alert")).toHaveCount(0);
});

test("activity shows measured progress across navigation and disables repeated cancellation", async ({page}) => {
  let job:any={id:"qa-job",kind:"link",status:"running",message:"Checking North Assembly — Night Maps",started:Date.now()/1000,completed:5,total:10,eta_seconds:20,progress_updated_at:Date.now()/1000};
  let cancellations=0;
  await page.route("**/__test_rpc",async route=>{
    const request=route.request().postDataJSON();
    if(request.method==="job.cancel") {cancellations++;job={...job,status:"cancelling"};await route.fulfill({json:{result:true}});return;}
    const response=await rpc(request.method,request.args);
    if(["state","job.status"].includes(request.method) && response.result) response.result.online_job=job;
    await route.fulfill({json:response});
  });
  await page.goto("/");
  const indicator=page.getByRole("button",{name:"Show activity: Link releases",exact:true});
  await expect(indicator).toContainText("50%");
  await expect(indicator).toContainText("20s remaining");
  await indicator.click();
  await expect(page.getByRole("progressbar",{name:"Online actions progress"})).toHaveAttribute("value","50");
  await page.screenshot({path:"/tmp/tibrary-qa-activity.png"});
  await page.getByRole("button",{name:"Cancel task",exact:true}).click();
  await expect(page.getByRole("button",{name:"Cancelling…",exact:true})).toBeDisabled();
  expect(cancellations).toBe(1);
  await page.locator("aside").getByRole("button",{name:"Overview",exact:true}).click();
  await expect(indicator).toContainText("Cancelling");
});

test("a failed table request can be retried without freezing navigation", async ({page})=>{
  let failed=false;
  await page.route("**/__test_rpc",async route=>{
    const request=route.request().postDataJSON();
    if(!failed && request.method==="table" && request.args.route==="missing") {failed=true;await route.fulfill({json:{error:"Temporary catalogue failure"}});return;}
    await route.fulfill({json:await rpc(request.method,request.args)});
  });
  await page.goto("/");
  await page.locator("aside").getByRole("button",{name:"Missing releases",exact:true}).click();
  await expect(page.getByRole("alert")).toContainText("Temporary catalogue failure");
  await page.locator("aside").getByRole("button",{name:"Overview",exact:true}).click();
  await page.locator("aside").getByRole("button",{name:"Missing releases",exact:true}).click();
  await expect(page.locator(".table-scroll")).toHaveAttribute("aria-busy","false");
  await expect(page.locator("tbody tr").first()).toBeVisible();
});


test("General contains connection and separate template/audio cards; artist review and unlink stay consistent", async ({page}) => {
  await page.goto("/");
  const sidebar = page.locator("aside");
  await sidebar.getByRole("button", {name:"General", exact:true}).click();
  await expect(sidebar.getByRole("button",{name:"Connections",exact:true})).toHaveCount(0);
  await expect(sidebar.getByRole("button",{name:"Downloads & files",exact:true})).toHaveCount(0);
  await expect(page.getByRole("heading",{name:"Connection",exact:true})).toBeVisible();
  const connection = page.locator("#connection-settings");
  const connectBox = await connection.getByRole("button",{name:"Connect account",exact:true}).boundingBox();
  const testBox = await connection.getByRole("button",{name:"Test connections",exact:true}).boundingBox();
  expect(Math.abs(connectBox!.y - testBox!.y)).toBeLessThanOrEqual(1);
  const referenceHeight = (await page.locator(".template-reference").boundingBox())!.height;
  const inputHeight = (await page.getByLabel("Folder template",{exact:true}).boundingBox())!.height;
  expect(Math.abs(referenceHeight - inputHeight)).toBeLessThanOrEqual(2);
  await page.getByText("Template reference & tag variables",{exact:true}).click();
  await expect(page.locator(".template-token")).toHaveCount(9);
  await expect(page.locator(".template-reference")).not.toContainText("Default:");
  await expect(page.locator(".card").filter({has:page.getByRole("heading",{name:"Audio and file options",exact:true})})).not.toContainText("Folder template");
  if (process.env.TIBRARY_SETTINGS_SCREENSHOT) await page.screenshot({path:"/tmp/tibrary-general-beta33.png"});
  await sidebar.getByRole("button",{name:"Link artists",exact:true}).click();
  await expect(page.locator("tbody tr")).toHaveCount(1);
  const artist = (await rpc("table",{route:"artists",root:join(folder,"music")})).result.rows[0].id;
  await rpc("artists.choose",{artist,ids:["900001"]});
  const needsReview=(await rpc("table",{route:"artists",root:join(folder,"music"),filter:"review"})).result;
  expect(needsReview.total).toBe(0);
  await columnOnly(page, "Match status", ["Confirmed"]);
  await expect(page.locator("tbody tr")).toHaveCount(1);
  await page.locator("tbody tr").first().click({button:"right"});
  await expect(page.getByRole("menuitem",{name:"Recheck selected artists"})).toBeVisible();
  await page.getByRole("menuitem",{name:"Unlink selected artists"}).click();
  await columnAll(page,"Match status");
  await expect(page.locator("tbody tr")).toContainText("Unresolved");
  expect((await rpc("turso.stats",{root:join(folder,"music")})).result.track_count).toBe(2);
});

test("favourite artist states have distinct theme-aware colours", async ({page}) => {
  await page.route("**/__test_rpc", async route => {
    const request = route.request().postDataJSON();
    if (request.method === "table" && request.args.route === "favourites") {
      await route.fulfill({json:{result:{total:3,rows:[
        {id:"1",artist:"Local Artist",status:"Local only",tracks:2},
        {id:"2",artist:"Owned Artist",status:"In library",tracks:3},
        {id:"3",artist:"New Artist",status:"Missing locally",tracks:0},
      ]}}});
    } else await route.fulfill({json:await rpc(request.method,request.args)});
  });
  await page.goto("/");
  await page.locator("aside").getByRole("button",{name:"Favourite artists",exact:true}).click();
  const statusBadges=page.locator("tbody tr td:nth-child(3) .badge");
  await expect(statusBadges).toHaveCount(3);
  for (const theme of ["light","dark"]) {
    await page.evaluate(theme => {document.documentElement.dataset.theme=theme},theme);
    const colours = await statusBadges.evaluateAll(elements => elements.map(el => getComputedStyle(el).color));
    const muted = await statusBadges.first().evaluate(el => getComputedStyle(el).getPropertyValue("--muted"));
    expect(new Set(colours).size).toBe(3);
    expect(colours.every(colour => colour !== muted)).toBe(true);
  }
});

test("completed job activity exposes persistent per-item details without expansion", async ({page}) => {
  await page.goto("/");
  const job = (await rpc("job.start",{kind:"scan",args:{root:join(folder,"music"),force:true}})).result;
  await expect.poll(async()=> (await rpc("job.status")).result.job?.status).toBe("complete");
  let history:any[]=[];
  await expect.poll(async()=> {
    const saved=(await rpc("logs.history",{limit:500})).result.entries;
    history=saved.filter((entry:any)=>entry.job_id === job.id);
    return history.some((entry:any)=>entry.job_status === "complete");
  }).toBe(true);
  expect(history.length).toBeGreaterThan(2);
  expect(history.every((entry:any)=>entry.job_id === job.id)).toBe(true);
  expect(history.some((entry:any)=>entry.message.includes("Reading local tags"))).toBe(true);
  await page.locator("aside").getByRole("button",{name:"Activity",exact:true}).click();
  const panel=page.getByRole("region",{name:"Activity log",exact:true});
  await expect(panel.locator(".log-row").filter({hasText:"Reading local tags"}).first()).toBeVisible();
  await expect(panel.locator(`[data-job-id="${job.id}"]`)).toHaveCount(history.length);
  await expect(panel.getByRole("button",{name:/saved history|more details/})).toHaveCount(0);
});


test("table headers remain opaque in light and dark themes, including dialogs", async ({page}) => {
  await page.goto("/");
  await page.locator("aside").getByRole("button",{name:"Link releases",exact:true}).click();
  await columnAll(page, "Status");
  for (const theme of ["light","dark"]) {
    await page.evaluate(theme => {document.documentElement.dataset.theme=theme},theme);
    const header=page.getByRole("columnheader").first();
    await expect(header).toBeVisible();
    expect(await header.evaluate(el=>getComputedStyle(el).backgroundColor)).toMatch(/^rgb\(/);
    expect(await header.evaluate(el=>getComputedStyle(el).opacity)).toBe("1");
  }
  await (await localTrackRow(page,"First Light")).dblclick();
  await page.getByText("All saved tags & DJ checks",{exact:true}).click();
  const header=page.getByRole("dialog").locator("th").first();
  expect(await header.evaluate(el=>getComputedStyle(el).backgroundColor)).toMatch(/^rgb\(/);
});


test("activity keeps its live progress row stable during updates", async ({page}) => {
  let message = "Checking file 1 of 20";
  const started=Date.now()/1000;
  await page.route("**/__test_rpc",async route=>{
    const request=route.request().postDataJSON();
    if (request.method === "logs.history") return route.fulfill({json:{result:{entries:[],next_before_id:null}}});
    const response=await rpc(request.method,request.args);
    if (["state","job.status"].includes(request.method) && response.result) {
      response.result.job={id:"local-live",kind:"scan",status:"running",message,started,completed:1,total:20};
      response.result.logs=[{at:new Date().toISOString(),message,category:"local",progress_id:"local-live",job_id:"local-live",job_kind:"scan",job_status:"running"}];
    }
    await route.fulfill({json:response});
  });
  await page.goto("/");
  await page.locator("aside").getByRole("button",{name:"Activity",exact:true}).click();
  const panel=page.getByRole("region",{name:"Activity log",exact:true});
  const row=panel.locator(".log-row");
  await expect(row).toHaveCount(1);
  await row.evaluate(element=>element.setAttribute("data-stability-check","retained"));
  const top=(await panel.boundingBox())!.y;
  message="Reading local tags · 12/20 files · An artist with a very long album name and an extended track title which needs to wrap across several lines without moving the panel";
  await expect(row).toContainText("12/20");
  await expect(row).toHaveAttribute("data-stability-check","retained");
  expect((await panel.boundingBox())!.y).toBe(top);
  await expect(panel.locator(".batch-toggle")).toHaveCount(0);
  await expect(panel.getByRole("button",{name:/saved history|more details/})).toHaveCount(0);
});

test("unified activity retains concurrent details through empty snapshots and cancels only the selected worker", async ({page}) => {
  const started=Date.now()/1000;
  const local={id:"group-local",kind:"apply",status:"running",message:"Updating local tags",started,completed:1,total:5};
  const online={id:"group-online",kind:"discography",status:"running",message:"Artist 2 of 10 — release details",started,completed:2,total:10};
  const download={id:"group-download",kind:"download",status:"running",message:"Downloading two releases",started,completed:0,total:2};
  const logs=[
    {at:new Date().toISOString(),message:"Applied an online title to a local file",category:"online",job_id:local.id,job_kind:local.kind,job_status:"running"},
    {at:new Date().toISOString(),message:"Download track details for an artist",category:"error",job_id:online.id,job_kind:online.kind,job_status:"running"},
    {at:new Date().toISOString(),message:"Transfer started",category:"general",job_id:download.id,job_kind:download.kind,job_status:"running"},
  ];
  const monitor=Object.fromEntries([1,2].flatMap(n=>[
    [`${download.id}:batch:r${n}`,{id:`r${n}`,kind:"batch",job_id:download.id,release_id:`r${n}`,artist:"Artist",release:`Release ${n}`,status:"running",total_tracks:1}],
    [`${download.id}:track:r${n}:t${n}`,{id:`t${n}`,kind:"track",job_id:download.id,release_id:`r${n}`,title:`Track ${n}`,status:"running",index:1,total_tracks:1,percent:25,bytes:1024,updated_at:started}],
  ]));
  let empty=false,emptyReplies=0,snapshots=0;
  const cancelled:string[]=[];
  let epochs=[0,0,0];
  await page.route("**/__test_rpc",async route=>{
    const request=route.request().postDataJSON();
    if (request.method === "logs.clear") {
      expect(request.args.stream).toBe("all");
      epochs=[1,1,1];
      return route.fulfill({json:{result:{cleared:true,activity_epochs:epochs}}});
    }
    if (request.method === "logs.history") return route.fulfill({json:{result:{entries:[],next_before_id:null}}});
    if (request.method === "job.cancel") {
      cancelled.push(request.args.kind);
      return route.fulfill({json:{result:{cancelled:true}}});
    }
    const response=await rpc(request.method,request.args);
    if (["state","job.status"].includes(request.method) && response.result) {
      Object.assign(response.result,{job:local,online_job:online,download_job:download,activity_epochs:[0,0,0],logs:empty?[]:logs,download_monitor:empty?{}:monitor});
      if (empty) emptyReplies++;
      snapshots++;
    }
    await route.fulfill({json:response});
  });
  await page.goto("/");
  await page.locator("aside").getByRole("button",{name:"Activity",exact:true}).click();
  const panel=page.getByRole("region",{name:"Activity log",exact:true});
  for (const entry of logs) await expect(panel.locator(".log-row").filter({hasText:entry.message})).toHaveCount(1);
  await expect(panel.locator(".log-row").filter({hasText:/Track [12] · running/})).toHaveCount(2);
  await page.getByRole("region",{name:"Online actions worker",exact:true}).getByRole("button",{name:"Cancel task",exact:true}).click();
  expect(cancelled).toEqual(["discography"]);
  await expect(page.getByRole("region",{name:"Local actions worker",exact:true}).getByRole("button",{name:"Cancel task",exact:true})).toBeEnabled();
  await expect(page.getByRole("region",{name:"Downloads worker",exact:true}).getByRole("button",{name:"Cancel task",exact:true})).toBeEnabled();
  empty=true;
  await expect.poll(()=>emptyReplies).toBeGreaterThan(0);
  for (const entry of logs) await expect(panel.locator(".log-row").filter({hasText:entry.message})).toHaveCount(1);
  await expect(panel.locator(".log-row").filter({hasText:/Track [12] · running/})).toHaveCount(2);
  // Old snapshots after Clear must not restore archived rows or transfers.
  empty=false;
  await panel.getByRole("button",{name:"Clear activity",exact:true}).click();
  const afterClear=snapshots;
  await expect.poll(()=>snapshots).toBeGreaterThan(afterClear);
  await expect(panel.locator(".log-row")).toHaveCount(0);
});

test("saved activity loads older pages and searches archived details without expansion", async ({page}) => {
  const entries=Array.from({length:1002},(_,i)=>({saved_id:i+1,at:new Date(Date.parse("2026-09-30T12:00:00Z")+i*1000).toISOString(),message:`Checked recording ${i+1}`,category:"online",job_id:"archive-refresh",job_kind:"discography",job_status:i===1001?"complete":"running"}));
  const cursors:(number|null)[]=[];
  await page.route("**/__test_rpc",async route=>{
    const request=route.request().postDataJSON();
    if (request.method === "logs.history") {
      cursors.push(request.args.before_id || null);
      const candidates=entries.filter(entry=>(!request.args.before_id || entry.saved_id < request.args.before_id) && (!request.args.search || entry.message.includes(request.args.search))).slice().reverse();
      const pageEntries=candidates.slice(0,500);
      return route.fulfill({json:{result:{entries:pageEntries,next_before_id:candidates.length>500 ? pageEntries.at(-1)!.saved_id : null}}});
    }
    const response=await rpc(request.method,request.args);
    if (request.method === "state" && response.result) Object.assign(response.result,{job:null,online_job:{id:"archive-refresh",kind:"discography",status:"complete",message:"Release refresh complete",started:1,historical:true},download_job:null,logs:[entries[1001]],download_monitor:{},activity_epochs:[0,0,0]});
    await route.fulfill({json:response});
  });
  await page.goto("/");
  await page.locator("aside").getByRole("button",{name:"Activity",exact:true}).click();
  const panel=page.getByRole("region",{name:"Activity log",exact:true});
  await expect(panel.locator(".log-row")).toHaveCount(500);
  await panel.locator(".activity-log").evaluate(element=>{element.scrollTop=element.scrollHeight;});
  await expect(panel.locator(".log-row")).toHaveCount(1000);
  await panel.locator(".activity-log").evaluate(element=>{element.scrollTop=element.scrollHeight;});
  await expect(panel.locator(".log-row")).toHaveCount(1002);
  expect(cursors).toEqual([null,503,3]);
  await panel.getByRole("searchbox").fill("Checked recording 10");
  await expect(panel.locator(".log-row")).toHaveCount(14);
  await expect(panel.locator(".log-row").first()).toContainText("Checked recording 1002");
  await expect(panel.locator(".batch-toggle")).toHaveCount(0);
  await page.unrouteAll({behavior:"wait"});
});
