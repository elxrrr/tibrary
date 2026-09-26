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
      },
    },
  );
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
  const screenshots: Record<string, string> = {"Prepare library":"prepare_library", "Link artists":"link_artists", "Link releases":"link_releases", "Missing releases":"missing_releases", "Local duplicates":"local_duplicates", "MQA audit":"mqa_audit", "Favourite artists":"favourite_artists", "Activity":"activity_log"};
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
    "Connections",
    "Downloads & files",
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
        await page.getByRole("button", {name:"Scan tags",exact:true}).click();
        await expect.poll(async () => (await rpc("job.status")).result?.job?.status).toBe("complete");
        await expect(page.getByRole("button", {name:"Rescan tags",exact:true})).toBeVisible();
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
  await page.getByRole("button", { name: "Coverage", exact: true }).click();
  await expect(childBox).toBeVisible();
  await expect(childBox).not.toBeChecked();
  await parent.check();
  await expect(childBox).toBeChecked();
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
    .getByRole("button", { name: "Downloads & files", exact: true })
    .click();
  await expect(
    page.getByRole("heading", { name: "Download & files", exact: true }),
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
  await page.locator("aside").getByRole("button",{name:"Connections",exact:true}).click();
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
  const missing = await rpc("table", {route:"missing",timeline:"All missing releases",status:"Missing release",limit:20,recommendation:"All recommendations"});
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
  await page.getByRole("button",{name:/^(Scan|Rescan) tags$/}).click();
  await expect.poll(async () => (await rpc("job.status")).result?.job?.status).toBe("complete");
  await expect(page.getByRole("button",{name:"Rescan tags"})).toBeVisible();
  await expect(page.getByRole("heading",{name:"Local duplicates",exact:true})).toBeVisible();
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
  await page.getByRole("button",{name:"Recheck cached releases",exact:true}).click();
  await expect.poll(async () => (await rpc("job.status")).result.online_job?.status).toBe("complete");
  const after = (await rpc("table",args)).result;
  expect(after.rows.map((row:any)=>row.id)).toEqual(before.rows.map((row:any)=>row.id));
  expect(after.missing_total).toBe(before.missing_total);
  const state = (await rpc("state")).result;
  expect(state.logs.some((log:any)=>log.category === "online" && log.message.includes("Cached release check complete"))).toBe(true);
  expect(state.online_job.message).toContain("music files unchanged");
});
