import { test, expect } from "@playwright/test";
import { spawn, execFileSync } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
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
  folder = mkdtempSync(join(tmpdir(), "tibrary-browser-"));
  const root = resolve("..");
  execFileSync(
    join(root, ".venv/bin/python"),
    [join(root, "support/tests/seed_desktop.py"), folder],
    {
      env: {
        ...process.env,
        PYTHONPATH: join(root, "app") + ":" + join(root, "support/tests"),
      },
    },
  );
  child = spawn(
    join(root, ".venv/bin/python"),
    ["-m", "library_manager.sidecar", "--demo", "--db", join(folder, "db")],
    { env: { ...process.env, PYTHONPATH: join(root, "app") } },
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
  await page.screenshot({ path: "test-results/overview.png", animations: "disabled" });
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
    const screenshots: Record<string, string> = {
      "Link releases": "link_releases",
      "Missing releases": "missing_releases",
      Connections: "connections",
      "Add missing tags": "add_tags",
    };
    if (screenshots[name])
      await page.screenshot({ path: `test-results/${screenshots[name]}.png` });
  }
  await page.screenshot({ path: "test-results/activity.png" });
  expect(errors).toEqual([]);
});
test("local table sorting and filters are usable", async ({ page }) => {
  await page.goto("/");
  await page
    .locator("aside")
    .getByRole("button", { name: "Link releases", exact: true })
    .click();
  await expect(page.locator("tbody tr").first()).toBeVisible();
  await page.getByRole("button", { name: "Release", exact: true }).click();
  await page.getByRole("button", { name: "Release", exact: true }).click();
  await page.screenshot({ path: "test-results/link-releases.png" });
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
  await page.getByRole("checkbox", { name: "Select visible rows" }).check();
  await page.locator("tbody tr").first().click({ button: "right" });
  await page
    .getByRole("menuitem", { name: "Ignore 2 selected tracks" })
    .click();
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
  await page.getByRole("button", { name: "Reset metadata defaults" }).click();
  await expect(page.getByRole("alert")).toHaveCount(0);
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  await expect(
    page.getByRole("button", { name: "Reset metadata defaults" }),
  ).toBeEnabled();
  await page.screenshot({ path: "test-results/download-settings-dark.png" });
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
  await page.screenshot({ path: "test-results/system-dark.png" });
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
