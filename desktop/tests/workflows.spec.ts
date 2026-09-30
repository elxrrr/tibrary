import { test, expect } from "@playwright/test";
import { spawn, execFileSync } from "node:child_process";
import { mkdtempSync, rmSync, realpathSync } from "node:fs";
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
test.afterEach(async () => {
  child.stdin.end();
  await new Promise<void>((resolve) => child.on("exit", () => resolve()));
  rmSync(folder, { recursive: true, force: true });
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
    page.getByRole("heading", { name: "Overview", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Working", exact: true }),
  ).toHaveCount(0);
  const screenshots: Record<string, string> = {"Prepare library":"prepare_library", "Link artists":"link_artists", "Link releases":"link_releases", "Missing releases":"missing_releases", "Local duplicates":"local_duplicates", "MQA audit":"mqa_audit", "Favourite artists":"favourite_artists", "Activity":"activity_log", "General":"general_settings"};
  if (process.env.TIBRARY_SCREENSHOTS) {
    await expect(page.getByRole("region",{name:"Latest missing releases"}).locator(".library-row").first()).toBeVisible();
    await page.screenshot({path:"docs/imgs/overview.png"});
  }
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
      page.getByRole("heading", { name, exact: true }).first(),
    ).toBeVisible();
    if (await page.locator(".table-scroll").count())
      await expect(page.locator(".table-scroll")).toHaveAttribute(
        "aria-busy",
        "false",
      );
    await expect(page.getByRole("alert")).toHaveCount(0);
    if (process.env.TIBRARY_SCREENSHOTS && screenshots[name]) {
      if (name === "Local duplicates") {
        await page.getByRole("button", {name:"Check local duplicates",exact:true}).click();
        await expect(page.locator(".header-workload")).toBeVisible();
        await expect.poll(async () => (await rpc("job.status")).result?.job?.status).toBe("complete");
        await expect(page.getByRole("button", {name:"Check local duplicates",exact:true})).toBeEnabled();
        await expect(page.locator(".header-workload")).toHaveCount(0);
      }
      await page.mouse.move(1590, 10);
      await page.screenshot({path:`docs/imgs/${screenshots[name]}.png`});
    }
  }
  expect(errors).toEqual([]);
});

test("window navigation remains available with the sidebar hidden and overview lists stay bounded", async ({page}) => {
  await rpc("queue.decision", {ids:["910002","910003","910004","910005","910006"],decision:"removed"});
  await page.goto("/");
  await expect(page.getByRole("button", {name:"Back",exact:true})).toBeDisabled();
  await page.locator("aside").getByRole("button", {name:"Prepare library",exact:true}).click();
  await page.locator("aside").getByRole("button", {name:"Link catalogue",exact:true}).click();
  await page.getByRole("button", {name:"Hide sidebar",exact:true}).click();
  await expect(page.locator("aside")).toBeHidden();
  expect((await page.locator("main").boundingBox())!.x).toBe(0);
  await page.getByRole("button", {name:"Back",exact:true}).click();
  await expect(page.getByRole("heading", {name:"Prepare library",exact:true})).toBeVisible();
  await page.getByRole("button", {name:"Forward",exact:true}).click();
  await expect(page.getByRole("heading", {name:"Link catalogue",exact:true})).toBeVisible();
  await page.getByRole("button", {name:"Back",exact:true}).click();
  await page.getByRole("button", {name:"Show sidebar",exact:true}).click();
  await page.locator("aside").getByRole("button", {name:"Overview",exact:true}).click();
  await expect(page.getByRole("button", {name:"Forward",exact:true})).toBeDisabled();
  const list = page.getByRole("region", {name:"Latest missing releases"});
  await expect(list.locator(".library-row").first()).toBeVisible();
  expect((await list.boundingBox())!.height).toBeLessThanOrEqual(320);
  expect(await list.evaluate(element => getComputedStyle(element).overflowY)).toBe("auto");
});
test("local table sorting and filters are usable", async ({ page }) => {
  await page.goto("/");
  await page
    .locator("aside")
    .getByRole("button", { name: "Link releases", exact: true })
    .click();
  await page.getByRole("combobox", { name: "Table filter" }).selectOption("all");
  await expect(page.locator("tbody tr").first()).toBeVisible();
  await page.getByRole("button", { name: "Release", exact: true }).click();
  await page.getByRole("button", { name: "Release", exact: true }).click();
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
  await childBox.uncheck();
  await expect(parent).toHaveJSProperty("indeterminate", true);
  await expect.poll(async () => (await rpc("queue.export",{format:"text"})).result?.text)
    .toBe("https://tidal.com/track/91000101\nhttps://tidal.com/track/91000102\n");
  await page.getByRole("button",{name:"Export",exact:true}).click();
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
  await page.getByRole("combobox", { name: "Table filter" }).selectOption("all");
  await expect(page.locator("tbody tr").first()).toBeVisible();
  await page.getByRole("checkbox", { name: "Select visible rows" }).check();
  await page.locator("tbody tr").first().click({ button: "right" });
  await page
    .getByRole("menuitem", { name: "Ignore 2 selected tracks" })
    .click();
  await page.getByRole("combobox", { name: "Table filter" }).selectOption("unlinked");
  await expect(page.locator("tbody tr")).toHaveCount(0);
  await page
    .getByRole("combobox", { name: "Table filter" })
    .selectOption("ignored");
  await expect(page.locator("tbody tr")).toHaveCount(2);
  await expect(page.getByRole("alert")).toHaveCount(0);
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
    page.getByRole("heading", { name: "General", exact: true }),
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
  await page
    .getByRole("combobox", { name: "Release timeline" })
    .selectOption("All missing releases");
  await page
    .getByRole("button", { name: "Expand Blue Hours", exact: true })
    .click();
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
  await page.getByRole("button", { name: /Track & disc numbers/ }).click();
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

test("activity has independent online, local and download panels", async ({page}) => {
  await page.goto("/");
  await page.locator("aside").getByRole("button", {name:"Activity",exact:true}).click();
  await expect(page.getByRole("region", {name:"Online actions"})).toBeVisible();
  await expect(page.getByRole("region", {name:"Local actions"})).toBeVisible();
  await expect(page.getByRole("region", {name:"Downloads"})).toBeVisible();
  await expect(page.locator(".activity-status small")).toHaveText(["Online actions", "Local actions", "Downloads"]);
  await expect(page.locator(".activity-status h2")).toHaveText(["Awaiting task...", "Awaiting task...", "Awaiting task..."]);
  expect((await page.getByRole("region", {name:"Downloads"}).boundingBox())?.height).toBeGreaterThan(500);
  await page.getByRole("searchbox", {name:"Search local actions"}).fill("nothing matches");
  await expect(page.getByRole("region", {name:"Online actions"}).getByRole("searchbox")).toHaveValue("");
  await page.getByRole("button", {name:"Clear local actions"}).click();
  await expect(page.getByRole("region", {name:"Downloads"})).toBeVisible();
});

test("display highlight preference changes focus palette", async ({page}) => {
  await page.goto("/");
  await page.locator("aside").getByRole("button", {name:"General",exact:true}).click();
  await page.getByLabel("Highlight colour").selectOption("grey");
  await expect(page.locator("html")).toHaveAttribute("data-highlight", "grey");
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
  await expect(page.getByRole("heading", {name:"Overview",exact:true})).toBeVisible();
  await expect(page.getByRole("alert")).toHaveCount(0);
  const missing = await rpc("table", {route:"missing",timeline:"All missing releases",status:"all",limit:20,recommendation:"My album artists"});
  await expect(page.locator(".metric").filter({hasText:"Missing releases"})).toContainText(String(missing.result.total));
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
  await page.getByRole("combobox",{name:"Table filter"}).selectOption("all");
  await page.locator("tbody tr").first().dblclick();
  await page.getByText("All saved tags & DJ checks",{exact:true}).click();
  await expect(page.getByRole("table",{name:"Local file tags"})).toBeVisible();
  await expect(page.getByRole("dialog").locator("pre")).toHaveCount(0);
});

test("automatic correction previews can be applied and all-files view remains available", async ({page}) => {
  await page.goto("/");
  await page.locator("aside").getByRole("button",{name:"Correct tags",exact:true}).click();
  await page.getByRole("button",{name:/Track & disc numbers/}).click();
  await expect(page.locator("tbody tr")).toHaveCount(2);
  await page.getByRole("checkbox",{name:"Select visible rows"}).check();
  await page.getByRole("button",{name:/Review & apply/}).click();
  await expect(page.getByRole("dialog").getByRole("table",{name:"Tag comparison"}).first()).toBeVisible();
  await page.getByRole("button",{name:"Confirm & continue"}).click();
  await expect.poll(async()=> (await rpc("job.status")).result?.job?.status).toBe("complete");
  await expect(page.getByRole("alert")).toHaveCount(0);
  await expect(page.locator("tbody tr")).toHaveCount(0);
  await page.getByRole("combobox",{name:"Table filter"}).selectOption("all");
  await expect(page.locator("tbody tr")).toHaveCount(2);
});

test("organise files previews and applies only to the disposable library", async ({page}) => {
  await page.goto("/");
  await page.locator("aside").getByRole("button",{name:"Organise files",exact:true}).click();
  await page.getByRole("button",{name:"Preview moves"}).click();
  await expect.poll(async () => (await rpc("job.status")).result?.job?.status).toBe("complete");
  await expect(page.locator("tbody tr").first()).toBeVisible();
  await page.locator("tbody tr").first().dblclick();
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

test("MQA and local duplicate scan controls complete without blocking navigation", async ({page}) => {
  await page.goto("/");
  await page.locator("aside").getByRole("button",{name:"MQA audit",exact:true}).click();
  await expect(page.locator("tbody").getByText("Not audited").first()).toBeVisible();
  await page.getByRole("button",{name:/Recheck|Audit/}).first().click();
  await expect.poll(async () => (await rpc("job.status")).result?.job?.status).toBe("complete");
  await page.locator("aside").getByRole("button",{name:"Local duplicates",exact:true}).click();
  await page.getByRole("button",{name:"Check local duplicates",exact:true}).click();
  await expect.poll(async () => (await rpc("job.status")).result?.job?.status).toBe("complete");
  await expect(page.getByRole("button",{name:"Check local duplicates",exact:true})).toBeVisible();
  await expect(page.getByRole("heading",{name:"Local duplicates",exact:true})).toBeVisible();
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
  await expect(page.getByRole("button",{name:"Check local changes",exact:true})).toBeDisabled();
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
  await page.getByText("More update options",{exact:true}).click();
  await page.getByRole("button",{name:"Recalculate saved results",exact:true}).click();
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
  await page.getByRole("combobox",{name:"Release type",exact:true}).selectOption("EP");
  await expect(page.locator("tbody")).toContainText("Between Stations");
  await expect(page.locator("tbody")).not.toContainText("Night Maps");
  await page.getByRole("combobox",{name:"Release type",exact:true}).selectOption("All types");
  await page.getByRole("textbox",{name:"Filter table",exact:true}).fill("Night Maps");
  await expect(page.locator("tbody")).toContainText("Night Maps");
  await expect(page.locator("tbody")).not.toContainText("Between Stations");
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
  await page.getByRole("combobox",{name:"Release timeline"}).selectOption("All missing releases");
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
  await page.getByRole("combobox",{name:"Release timeline"}).selectOption("All missing releases");
  await expect(page.locator("tbody")).not.toContainText("Private Weather");
  await page.getByRole("combobox",{name:"Table filter"}).selectOption("Unavailable");
  await expect(page.locator("tbody")).toContainText("Private Weather");
  await expect(page.getByRole("combobox",{name:"Release timeline"})).toHaveValue("All missing releases");
  await page.getByRole("combobox",{name:"Table filter"}).selectOption("all");
  await page.getByRole("checkbox",{name:"Select Blue Hours",exact:true}).check();
  await page.getByRole("button",{name:"Check availability",exact:true}).click();
  await expect.poll(()=>checks.length).toBe(1);
  expect(checks[0].ids).toEqual(["910001"]);
  expect(checks[0].force).toBeUndefined();
  await expect(page.getByRole("button",{name:"Check availability",exact:true})).toBeDisabled();
  await sidebar.getByRole("button",{name:"Overview",exact:true}).click();
  await expect(page.getByRole("button",{name:"Show activity: Check release availability",exact:true})).toBeVisible();
  job={...job,status:"complete",completed:1,message:"Availability check complete · cached results saved"};
  await sidebar.getByRole("button",{name:"Missing releases",exact:true}).click();
  await expect(page.getByRole("button",{name:"Check availability",exact:true})).toBeEnabled();
  const visible=(await rpc("table",{route:"missing",timeline:"All missing releases",recommendation:"My album artists",filter:"all",limit:50})).result.rows;
  await page.getByRole("button",{name:"Check availability",exact:true}).click();
  await expect.poll(()=>checks.length).toBe(2);
  expect([...checks[1].ids].sort()).toEqual(visible.map((row:any)=>row.id).sort());
  await page.getByRole("button",{name:"Expand Blue Hours",exact:true}).click();
  await page.getByRole("button",{name:"Actions for track First Light",exact:true}).click();
  await page.getByRole("menuitem",{name:"Check release availability",exact:true}).click();
  await expect.poll(()=>checks.length).toBe(3);
  expect(checks[2].ids).toEqual(["910001"]);
  expect(checks[2].force).toBe(true);
  await page.getByText("More update options",{exact:true}).click();
  await page.getByRole("button",{name:"Check saved release availability",exact:true}).click();
  await expect.poll(()=>checks.length).toBe(4);
  expect(checks[3].ids).toBeUndefined();
  expect(checks[3].force).toBeUndefined();
  await page.getByText("More update options",{exact:true}).click();
  await page.getByRole("button",{name:"Recheck saved availability online",exact:true}).click();
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
  await page.getByRole("combobox",{name:"Table filter"}).selectOption("review");
  await expect(page.locator("tbody tr")).toHaveCount(0);
  await page.getByRole("combobox",{name:"Table filter"}).selectOption("all");
  await expect(page.locator("tbody tr")).toHaveCount(1);
  await page.locator("tbody tr").first().click({button:"right"});
  await expect(page.getByRole("menuitem",{name:"Recheck selected artists"})).toBeVisible();
  await page.getByRole("menuitem",{name:"Unlink selected artists"}).click();
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
  await expect(page.locator(".badge")).toHaveCount(3);
  for (const theme of ["light","dark"]) {
    await page.evaluate(theme => {document.documentElement.dataset.theme=theme},theme);
    const colours = await page.locator(".badge").evaluateAll(elements => elements.map(el => getComputedStyle(el).color));
    const muted = await page.locator(".badge").first().evaluate(el => getComputedStyle(el).getPropertyValue("--muted"));
    expect(new Set(colours).size).toBe(3);
    expect(colours.every(colour => colour !== muted)).toBe(true);
  }
});

test("completed job activity stays grouped with persistent per-item details", async ({page}) => {
  await page.goto("/");
  const job = (await rpc("job.start",{kind:"scan",args:{root:join(folder,"music"),force:true}})).result;
  await expect.poll(async()=> (await rpc("job.status")).result.job?.status).toBe("complete");
  const history = (await rpc("logs.job",{id:job.id})).result;
  expect(history.length).toBeGreaterThan(2);
  expect(history.every((entry:any)=>entry.job_id === job.id)).toBe(true);
  expect(history.some((entry:any)=>entry.message.includes("Reading local tags"))).toBe(true);
  await page.locator("aside").getByRole("button",{name:"Activity",exact:true}).click();
  const panel=page.getByRole("region",{name:"Local actions",exact:true});
  const toggle=panel.getByRole("button",{name:/Scan library.*complete/}).first();
  await expect(toggle).toBeVisible();
  await expect(panel.locator(".batch-children")).toHaveCount(0);
  await toggle.click();
  await expect(panel.locator(".batch-children")).toContainText("Reading local tags");
  await expect(panel.getByRole("button",{name:/Load (full saved history|more details)/})).toHaveCount(0);
  await expect(panel.locator(".batch-children .log-row")).toHaveCount(history.length);
});


test("table headers remain opaque in light and dark themes, including dialogs", async ({page}) => {
  await page.goto("/");
  await page.locator("aside").getByRole("button",{name:"Link releases",exact:true}).click();
  await page.getByRole("combobox",{name:"Table filter"}).selectOption("all");
  for (const theme of ["light","dark"]) {
    await page.evaluate(theme => {document.documentElement.dataset.theme=theme},theme);
    const header=page.getByRole("columnheader").first();
    await expect(header).toBeVisible();
    expect(await header.evaluate(el=>getComputedStyle(el).backgroundColor)).toMatch(/^rgb\(/);
    expect(await header.evaluate(el=>getComputedStyle(el).opacity)).toBe("1");
  }
  await page.locator("tbody tr").first().dblclick();
  await page.getByText("All saved tags & DJ checks",{exact:true}).click();
  const header=page.getByRole("dialog").locator("th").first();
  expect(await header.evaluate(el=>getComputedStyle(el).backgroundColor)).toMatch(/^rgb\(/);
});


test("local activity keeps its layout and expanded live row stable during updates", async ({page}) => {
  let message = "Checking file 1 of 20";
  const started=Date.now()/1000;
  await page.route("**/__test_rpc",async route=>{
    const request=route.request().postDataJSON();
    const response=await rpc(request.method,request.args);
    if (["state","job.status"].includes(request.method) && response.result) {
      response.result.job={id:"local-live",kind:"scan",status:"running",message,started,completed:1,total:20};
      response.result.logs=[{at:new Date().toISOString(),message,category:"local",progress_id:"local-live",job_id:"local-live",job_kind:"scan",job_status:"running"}];
    }
    await route.fulfill({json:response});
  });
  await page.goto("/");
  await page.locator("aside").getByRole("button",{name:"Activity",exact:true}).click();
  const panel=page.getByRole("region",{name:"Local actions",exact:true});
  await panel.locator(".batch-toggle").click();
  const row=panel.locator(".log-row").last();
  await row.evaluate(element=>element.setAttribute("data-stability-check","retained"));
  const top=(await panel.boundingBox())!.y;
  message="Reading local tags · 12/20 files · An artist with a very long album name and an extended track title which needs to wrap across several lines without moving the panel";
  await expect(row).toContainText("12/20");
  await expect(row).toHaveAttribute("data-stability-check","retained");
  expect((await panel.boundingBox())!.y).toBe(top);
  await expect(panel.locator(".batch-toggle")).toHaveAttribute("aria-expanded","true");
  await expect(panel.getByRole("button",{name:/saved history|more details/})).toHaveCount(0);
});
