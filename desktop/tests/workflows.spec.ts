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
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await page.goto("/");
  await expect(
    page.getByRole("heading", { name: "Overview", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Working", exact: true }),
  ).toHaveCount(0);
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
    "Downloads",
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
  }
  expect(errors).toEqual([]);
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
    .getByRole("button", { name: "Downloads", exact: true })
    .click();
  await expect(
    page.getByRole("heading", { name: "Download engine", exact: true }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Reset download defaults" }).click();
  await expect(page.getByRole("alert")).toHaveCount(0);
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  await expect(
    page.getByRole("button", { name: "Reset download defaults" }),
  ).toBeEnabled();
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
  await expect.poll(async () => (await rpc("job.status")).result?.job?.status).toBe("failed");
  const after = await rpc("table", {route: "queue"});
  expect(after.result.rows).toEqual(before.result.rows);
});
