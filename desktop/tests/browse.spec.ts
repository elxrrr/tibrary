import { test, expect, Page } from "@playwright/test";
import {
  spawn,
  execFileSync,
  ChildProcessWithoutNullStreams,
} from "node:child_process";
import { mkdtempSync, rmSync, realpathSync } from "node:fs";
import { tmpdir } from "node:os";
import { resolve, join } from "node:path";
import { createInterface } from "node:readline";

// Only catalogue responses are fixtures. Queue mutations go through the real
// Rust RPC server and SQLite, with no account access or writes to user files.
const artist = { id: "600001", name: "North Assembly", picture: null };
const tracks = ["First Light", "Drift", "Low Tide"].map((title, index) => ({
  id: `6100010${index}`,
  title,
  artist: artist.name,
  artists: [artist],
  disc_number: 1,
  track_number: index + 1,
  duration: 180 + index * 12,
  available: true,
  ai: false,
  isrc: `GBBROWSE${index}`,
}));
const release = {
  id: "610001",
  title: "Blue Hours",
  artist: artist.name,
  artist_id: artist.id,
  artists: [artist],
  date: "2026-10-01",
  release_date: "2026-10-01",
  type: "ALBUM",
  release_type: "ALBUM",
  track_count: 3,
  quality: "LOSSLESS",
  available: true,
  ai: false,
  tracks_loaded: true,
  tracks,
};
const secondRelease = {
  ...release,
  id: "610002",
  title: "Night Maps",
  date: "2026-09-20",
  release_date: "2026-09-20",
  track_count: 1,
  tracks: [{ ...tracks[0], id: "61000200", title: "Signal" }],
};
const stamp = () => ({
  source: "cache",
  cached_at: Math.floor(Date.now() / 1000),
  stale: false,
});
const home = () => ({
  ...stamp(),
  connected: true,
  market: "GB",
  artists: [artist],
  releases: [release, secondRelease],
  sections: [
    { id: "new", title: "New releases", releases: [release, secondRelease] },
    { id: "artists", title: "Your artists", artists: [artist] },
  ],
});
type Request = { method: string; args: Record<string, any> };
type Override = (request: Request) => Promise<any> | any;
let child: ChildProcessWithoutNullStreams,
  folder: string,
  serial = 0;
let override: Override | undefined;
let noLibrary = false;
let requests: Request[] = [];
const pending = new Map<
  number,
  { resolve: (response: any) => void; reject: (error: Error) => void }
>();
function rpc(method: string, args: Record<string, any> = {}) {
  return new Promise<any>((resolve, reject) => {
    const id = ++serial;
    pending.set(id, { resolve, reject });
    child.stdin.write(JSON.stringify({ id, method, args }) + "\n");
  });
}
function saveRelease(value: any) {
  execFileSync("python3", [
    "-c",
    `import json,sqlite3,sys,time
c=sqlite3.connect(sys.argv[1]); r=json.loads(sys.argv[2])
for k,v in [('browse:v1:GB:release:'+r['id'],{'checked_at':int(time.time()),'value':{'release':r}}),('tag-review:GB:'+r['id'],r)]:
 c.execute('INSERT OR REPLACE INTO app_preferences VALUES(?,?)',(k,json.dumps(v)))
c.commit()`,
    join(folder, "db"),
    JSON.stringify(value),
  ]);
}
async function savedQueue(id = release.id) {
  const response = await rpc("queue.preview");
  if (response.error) throw new Error(response.error);
  return response.result.find((row: any) => row.id === id);
}
async function browse(page: Page) {
  await page.goto("/");
  await page.getByRole("button", { name: "Browse music", exact: true }).click();
  await expect(
    page.getByRole("heading", { name: "New releases", exact: true }),
  ).toBeVisible();
}
async function search(page: Page, query: string) {
  await page
    .getByRole("searchbox", { name: "Search online music" })
    .fill(query);
  await page.getByRole("button", { name: "Search", exact: true }).click();
}
test.beforeEach(async ({ page }) => {
  override = undefined;
  noLibrary = false;
  requests = [];
  folder = realpathSync(mkdtempSync(join(tmpdir(), "tibrary-browse-qa-")));
  const root = resolve("..");
  execFileSync(
    "python3",
    [join(root, "desktop/tests/seed_desktop.py"), folder],
    { env: { ...process.env, TIBRARY_SCREENSHOTS: "" } },
  );
  saveRelease(release);
  saveRelease(secondRelease);
  child = spawn(
    join(root, "desktop/src-tauri/target/debug/tibrary"),
    ["--rpc", "--db", join(folder, "db")],
    { env: { ...process.env, TIBRARY_TEST_MODE: "1" } },
  );
  createInterface({ input: child.stdout }).on("line", (line) => {
    const response = JSON.parse(line);
    pending.get(response.id)?.resolve(response);
    pending.delete(response.id);
  });
  child.on("exit", () => {
    for (const item of pending.values())
      item.reject(new Error("Disposable RPC process exited"));
    pending.clear();
  });
  await expect
    .poll(async () => (await rpc("state")).result?.job?.status)
    .toBe("complete");
  const covers = await page.evaluate(() =>
    Array.from({ length: 6 }, (_, index) => {
      const canvas = document.createElement("canvas");
      canvas.width = 160;
      canvas.height = 160;
      const c = canvas.getContext("2d")!;
      const colours = [
        ["#0f253b", "#5a8195"],
        ["#3e2b34", "#9d777e"],
        ["#243d37", "#a1ae88"],
        ["#3b2848", "#8e789a"],
        ["#342e25", "#c0a781"],
        ["#243946", "#a1b3ba"],
      ];
      const [dark, light] = colours[index];
      const gradient = c.createLinearGradient(0, 0, 160, 160);
      gradient.addColorStop(0, dark);
      gradient.addColorStop(1, light);
      c.fillStyle = gradient;
      c.fillRect(0, 0, 160, 160);
      c.strokeStyle = "rgba(255,255,255,.2)";
      c.lineWidth = 1;
      for (let ring = 0; ring < 5; ring++) {
        c.beginPath();
        c.arc(100 - index * 7, 50 + index * 9, 22 + ring * 13, 0, Math.PI * 2);
        c.stroke();
      }
      c.fillStyle = "rgba(255,255,255,.92)";
      c.font = "bold 12px sans-serif";
      c.fillText("NORTH ASSEMBLY", 12, 128);
      c.font = "9px sans-serif";
      c.fillText(
        [
          "BLUE HOURS",
          "NIGHT MAPS",
          "OPEN WATER",
          "GLASS",
          "DISTANT ROOMS",
          "BETWEEN STATIONS",
        ][index],
        12,
        144,
      );
      return canvas.toDataURL("image/jpeg", 0.9);
    }),
  );
  await page.route("**/__test_rpc", async (route) => {
    const request: Request = route.request().postDataJSON();
    requests.push(request);
    const special = await override?.(request);
    if (special !== undefined) return route.fulfill({ json: special });
    if (request.method === "browse.home")
      return route.fulfill({ json: { result: home() } });
    if (request.method === "browse.search") {
      const items =
        request.args.kind === "artists"
          ? [artist]
          : request.args.kind === "tracks"
            ? [
                {
                  ...tracks[0],
                  release_id: release.id,
                  album_id: release.id,
                  album: release,
                },
              ]
            : [release];
      return route.fulfill({
        json: {
          result: {
            ...stamp(),
            items,
            total: items.length,
            offset: request.args.offset || 0,
            limit: 24,
            has_more: false,
          },
        },
      });
    }
    if (request.method === "browse.artist")
      return route.fulfill({
        json: {
          result: {
            ...stamp(),
            artist,
            releases: [release, secondRelease],
            total: 2,
            offset: 0,
            limit: 24,
            has_more: false,
          },
        },
      });
    if (request.method === "browse.release")
      return route.fulfill({
        json: {
          result: {
            ...stamp(),
            release:
              request.args.id === secondRelease.id ? secondRelease : release,
          },
        },
      });
    if (request.method === "artwork.thumbnail")
      return route.fulfill({
        json: {
          result: {
            src: covers[
              (((Number(request.args.release_id) - 610001) % 6) + 6) % 6
            ],
            cover_id: String(request.args.release_id),
            size: 160,
          },
        },
      });
    const response = await rpc(request.method, request.args);
    if (noLibrary && request.method === "state" && response.result)
      response.result.roots = [];
    return route.fulfill({ json: response });
  });
});
test.afterEach(async ({ page }) => {
  await page.unrouteAll({ behavior: "ignoreErrors" });
  await page.close();
  const exited = new Promise<void>((resolve) =>
    child.once("exit", () => resolve()),
  );
  child.stdin.end();
  await exited;
  rmSync(folder, { recursive: true, force: true });
});

test("browse without a library follows artist release tracks and queues exact selections", async ({
  page,
}) => {
  noLibrary = true;
  await browse(page);
  if (process.env.TIBRARY_SCREENSHOTS)
    await page.screenshot({
      path: "/private/tmp/tibrary-browse-home-dark.png",
    });
  await page
    .getByRole("button", { name: `Browse artist ${artist.name}`, exact: true })
    .click();
  await page
    .getByRole("button", {
      name: `Open release ${release.title} by ${artist.name}`,
      exact: true,
    })
    .click();
  if (process.env.TIBRARY_SCREENSHOTS) {
    await page.screenshot({
      path: "/private/tmp/tibrary-browse-release-dark.png",
    });
  }
  const all = page.getByRole("checkbox", {
    name: "Select all available tracks",
    exact: true,
  });
  await all.uncheck();
  await page
    .getByRole("checkbox", { name: "Select Drift", exact: true })
    .check();
  await page
    .getByRole("button", { name: "Refresh browse results", exact: true })
    .click();
  await expect
    .poll(
      () =>
        requests.filter(
          (request) =>
            request.method === "browse.release" &&
            request.args.refresh === true,
        ).length,
    )
    .toBe(1);
  await expect(
    page.getByRole("checkbox", { name: "Select Drift", exact: true }),
  ).toBeChecked();
  await expect(
    page.getByRole("checkbox", { name: "Select First Light", exact: true }),
  ).not.toBeChecked();
  await page.getByRole("button", { name: "Back", exact: true }).click();
  await expect(
    page.getByRole("heading", { name: artist.name, exact: true }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Forward", exact: true }).click();
  await expect(
    page.getByRole("checkbox", { name: "Select Drift", exact: true }),
  ).toBeChecked();
  await expect(
    page.getByRole("checkbox", { name: "Select First Light", exact: true }),
  ).not.toBeChecked();
  await page
    .getByRole("button", { name: "Queue selected tracks (1)", exact: true })
    .click();
  await expect(
    page.getByRole("button", { name: "Open download queue", exact: true }),
  ).toBeVisible();
  const saved = await savedQueue();
  expect(saved.approved).toBe(true);
  expect(saved.selected).toEqual([tracks[1].id]);
  expect(
    requests
      .filter((r) => r.method === "browse.queue")
      .map((r) => r.args.selection),
  ).toEqual([{ [release.id]: [tracks[1].id] }]);
  await page
    .getByRole("button", { name: "Open download queue", exact: true })
    .click();
  await expect(
    page.getByRole("button", { name: "Download queue", exact: true }),
  ).toHaveClass(/current/);
});

test("browse search types pagination refresh and history keep the requested view", async ({
  page,
}) => {
  override = (request) => {
    if (request.method !== "browse.search" || request.args.kind !== "albums")
      return undefined;
    const later = !!request.args.offset;
    return {
      result: {
        ...stamp(),
        items: later ? [secondRelease] : [release],
        total: 2,
        offset: later ? 1 : 0,
        limit: 1,
        has_more: !later,
      },
    };
  };
  await browse(page);
  await search(page, "North Assembly");
  await page.getByRole("button", { name: "Show more", exact: true }).click();
  await expect(
    page.getByRole("button", {
      name: `Open release ${secondRelease.title} by ${artist.name}`,
      exact: true,
    }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", {
      name: `Open release ${release.title} by ${artist.name}`,
      exact: true,
    }),
  ).toBeVisible();
  await page.getByRole("tab", { name: "Artists", exact: true }).click();
  await expect(
    page.getByRole("button", {
      name: `Browse artist ${artist.name}`,
      exact: true,
    }),
  ).toBeVisible();
  await page
    .getByRole("button", { name: `Browse artist ${artist.name}`, exact: true })
    .click();
  await page.getByRole("button", { name: "Back", exact: true }).click();
  await expect(
    page.getByRole("tab", { name: "Artists", exact: true }),
  ).toHaveAttribute("aria-selected", "true");
  await expect(
    page.getByRole("searchbox", { name: "Search online music" }),
  ).toHaveValue("North Assembly");
  await page
    .getByRole("button", { name: "Refresh browse results", exact: true })
    .click();
  await expect
    .poll(() =>
      requests.some(
        (r) =>
          r.method === "browse.search" &&
          r.args.kind === "artists" &&
          r.args.refresh === true,
      ),
    )
    .toBe(true);
  await page.getByRole("tab", { name: "Tracks", exact: true }).click();
  await expect(page.getByText("First Light", { exact: true })).toBeVisible();
});

test("browse retries catalogue failures and ignores results from a previous search", async ({
  page,
}) => {
  let slowResolve: ((response: any) => void) | undefined;
  let retry = false;
  override = (request) => {
    if (request.method !== "browse.search") return undefined;
    if (request.args.query === "slow")
      return new Promise((resolve) => {
        slowResolve = resolve;
      });
    if (request.args.query === "broken" && !retry) {
      retry = true;
      return { error: "Catalogue temporarily unavailable. Try again." };
    }
    return {
      result: {
        ...stamp(),
        items: [secondRelease],
        total: 1,
        offset: 0,
        limit: 24,
        has_more: false,
      },
    };
  };
  await browse(page);
  await search(page, "broken");
  await expect(
    page.getByText("Catalogue temporarily unavailable. Try again.", {
      exact: true,
    }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Retry", exact: true }).click();
  await expect(
    page.getByRole("button", {
      name: `Open release ${secondRelease.title} by ${artist.name}`,
      exact: true,
    }),
  ).toBeVisible();
  await search(page, "slow");
  await expect.poll(() => !!slowResolve).toBe(true);
  await search(page, "newer");
  await expect(
    page.getByRole("button", {
      name: `Open release ${secondRelease.title} by ${artist.name}`,
      exact: true,
    }),
  ).toBeVisible();
  slowResolve!({
    result: {
      ...stamp(),
      items: [release],
      total: 1,
      offset: 0,
      limit: 24,
      has_more: false,
    },
  });
  await expect(
    page.getByRole("button", {
      name: `Open release ${release.title} by ${artist.name}`,
      exact: true,
    }),
  ).toHaveCount(0);
});

test("browse excludes unavailable and AI tracks from selection and prevents repeated queue actions", async ({
  page,
}) => {
  const guarded = {
    ...release,
    tracks: [
      tracks[0],
      { ...tracks[1], available: false },
      { ...tracks[2], ai: true },
    ],
  };
  let finish: ((result: any) => void) | undefined;
  override = (request) => {
    if (request.method === "browse.release")
      return { result: { ...stamp(), release: guarded } };
    if (request.method === "browse.queue")
      return new Promise((resolve) => {
        finish = resolve;
      });
  };
  await browse(page);
  await page
    .getByRole("button", {
      name: `Open release ${release.title} by ${artist.name}`,
      exact: true,
    })
    .click();
  await expect(
    page.getByRole("checkbox", { name: "Select Drift", exact: true }),
  ).toBeDisabled();
  await expect(
    page.getByRole("checkbox", { name: "Select Low Tide", exact: true }),
  ).toBeDisabled();
  await expect(
    page.getByRole("checkbox", { name: "Select First Light", exact: true }),
  ).toBeChecked();
  await page
    .getByRole("button", { name: "Queue selected tracks (1)", exact: true })
    .click();
  await expect.poll(() => !!finish).toBe(true);
  await expect(
    page.getByRole("button", { name: /Queue selected tracks/ }),
  ).toBeDisabled();
  expect(requests.filter((r) => r.method === "browse.queue")).toHaveLength(1);
  finish!({ result: { queued_releases: 1, queued_tracks: 1 } });
  await expect(
    page.getByRole("button", { name: "Open download queue", exact: true }),
  ).toBeVisible();
});

test("browse full release queue merges existing track selection without changing catalogue scope", async ({
  page,
}) => {
  const before = await rpc("table", { route: "missing", limit: 500 });
  await browse(page);
  await page
    .getByRole("button", {
      name: `Open release ${release.title} by ${artist.name}`,
      exact: true,
    })
    .click();
  await page
    .getByRole("checkbox", { name: "Select all available tracks", exact: true })
    .uncheck();
  await page
    .getByRole("checkbox", { name: "Select Drift", exact: true })
    .check();
  await page
    .getByRole("button", { name: "Queue selected tracks (1)", exact: true })
    .click();
  await expect(
    page.getByRole("button", { name: "Open download queue", exact: true }),
  ).toBeVisible();
  await page
    .getByRole("button", { name: "Queue full release", exact: true })
    .click();
  await expect
    .poll(async () => (await savedQueue())?.selected ?? null)
    .toBeNull();
  const saved = await savedQueue();
  expect(saved.approved).toBe(true);
  expect(saved.children).toHaveLength(3);
  expect(
    (await rpc("table", { route: "missing", limit: 500 })).result?.total,
  ).toBe(before.result?.total);
});

test("browse stays interactive while an independent catalogue job runs", async ({
  page,
}) => {
  override = async (request) => {
    if (request.method !== "state") return undefined;
    const response = await rpc(request.method, request.args);
    response.result.online_job = {
      id: "separate-refresh",
      kind: "discography",
      status: "running",
      message: "North Assembly · 3/20 artists checked",
      started: Date.now() / 1000,
      completed: 3,
      total: 20,
      percent: 15,
      eta_seconds: 45,
    };
    return response;
  };
  await browse(page);
  await search(page, "Blue Hours");
  await page
    .getByRole("button", {
      name: `Open release ${release.title} by ${artist.name}`,
      exact: true,
    })
    .click();
  const queue = page.getByRole("button", {
    name: "Queue full release",
    exact: true,
  });
  await expect(queue).toBeEnabled();
  await queue.click();
  await expect(
    page.getByRole("button", { name: "Open download queue", exact: true }),
  ).toBeVisible();
  expect((await savedQueue()).approved).toBe(true);
});

test("browse accepts a track link and preselects only that track", async ({
  page,
}) => {
  override = (request) =>
    request.method === "browse.resolve"
      ? { result: { kind: "release", id: release.id, track_id: tracks[1].id } }
      : undefined;
  await browse(page);
  await search(page, `https://tidal.com/browse/track/${tracks[1].id}`);
  await expect(
    page.getByRole("checkbox", { name: "Select Drift", exact: true }),
  ).toBeChecked();
  await expect(
    page.getByRole("checkbox", { name: "Select First Light", exact: true }),
  ).not.toBeChecked();
  await expect(
    page.getByRole("checkbox", { name: "Select Low Tide", exact: true }),
  ).not.toBeChecked();
  await page
    .getByRole("button", { name: "Queue selected tracks (1)", exact: true })
    .click();
  await expect(
    page.getByRole("button", { name: "Open download queue", exact: true }),
  ).toBeVisible();
  expect((await savedQueue()).selected).toEqual([tracks[1].id]);
});

test("browse shows saved music when disconnected and routes connection action to settings", async ({
  page,
}) => {
  override = (request) =>
    request.method === "browse.home"
      ? { result: { ...home(), connected: false, stale: true } }
      : undefined;
  await browse(page);
  await expect(
    page.getByText("Showing saved catalogue information.", { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", {
      name: `Open release ${release.title} by ${artist.name}`,
      exact: true,
    }),
  ).toBeEnabled();
  await page
    .getByRole("button", { name: "Connect account", exact: true })
    .click();
  await expect(
    page.getByRole("button", { name: "General", exact: true }),
  ).toHaveClass(/current/);
});

test("browse back retains loaded search pages and scroll position", async ({
  page,
}) => {
  const items = Array.from({ length: 26 }, (_, i) => ({
    ...release,
    id: String(620000 + i),
    title: `Release ${String(i + 1).padStart(2, "0")}`,
  }));
  override = (request) => {
    if (request.method === "browse.search") {
      const offset = request.args.offset || 0;
      return {
        result: {
          ...stamp(),
          items: items.slice(offset, offset + 13),
          total: 26,
          offset,
          limit: 13,
          has_more: offset === 0,
        },
      };
    }
    if (request.method === "browse.release")
      return {
        result: {
          ...stamp(),
          release: items.find((item) => item.id === request.args.id),
        },
      };
  };
  await browse(page);
  await search(page, "North Assembly");
  await page.getByRole("button", { name: "Show more", exact: true }).click();
  const card = page.getByRole("button", {
    name: `Open release Release 24 by ${artist.name}`,
    exact: true,
  });
  await card.scrollIntoViewIfNeeded();
  const offset = await page
    .locator(".browse-scroll")
    .evaluate((element) => element.scrollTop);
  expect(offset).toBeGreaterThan(0);
  const calls = requests.filter(
    (request) => request.method === "browse.search",
  ).length;
  await card.click();
  await expect(
    page.getByRole("heading", { name: "Release 24", exact: true }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Back", exact: true }).click();
  await expect(card).toBeVisible();
  await expect
    .poll(async () =>
      page.locator(".browse-scroll").evaluate((element) => element.scrollTop),
    )
    .toBeCloseTo(offset, 0);
  expect(
    requests.filter((request) => request.method === "browse.search"),
  ).toHaveLength(calls);
});

test("browse documentation screenshots use disposable catalogue artwork", async ({
  page,
}) => {
  test.skip(
    !process.env.TIBRARY_SCREENSHOTS,
    "Documentation capture is opt-in",
  );
  const titles = [
    "Blue Hours",
    "Night Maps",
    "Open Water",
    "Glass",
    "Distant Rooms",
    "Between Stations",
  ];
  const releases = titles.map((title, index) => ({
    ...release,
    id: String(610001 + index),
    title,
    date: `2026-10-${String(10 - index).padStart(2, "0")}`,
    type: index === 3 ? "SINGLE" : index === 5 ? "EP" : "ALBUM",
  }));
  const album = {
    ...release,
    track_count: 11,
    tracks: [
      "First Light",
      "Drift",
      "Low Tide",
      "Open Water",
      "Signal",
      "Afterimage",
      "Homeward",
      "Daybreak",
      "The Long Way",
      "Northbound",
      "Blue Hours",
    ].map((title, index) => ({
      ...tracks[0],
      id: String(61000100 + index),
      track_number: index + 1,
      title,
      duration: 175 + index * 17,
    })),
  };
  override = (request) => {
    if (request.method === "browse.home")
      return {
        result: {
          ...home(),
          sections: [
            { id: "new", title: "New releases", releases },
            {
              id: "favourites",
              title: "Your artists",
              artists: [
                artist,
                { id: "600002", name: "South Circuit" },
                { id: "600003", name: "Aster Vale" },
              ],
            },
          ],
        },
      };
    if (request.method === "browse.release")
      return { result: { ...stamp(), release: album } };
  };
  await browse(page);
  await expect(page.locator(".browse-release-card img")).toHaveCount(6);
  await page.screenshot({ path: "docs/imgs/browse_music.png" });
  await page
    .getByRole("button", {
      name: `Open release ${release.title} by ${artist.name}`,
      exact: true,
    })
    .click();
  await expect(page.locator(".browse-release-cover img")).toBeVisible();
  await page.screenshot({ path: "docs/imgs/browse_release.png" });
  await page
    .locator("aside")
    .getByRole("button", { name: "General", exact: true })
    .click();
  await page.getByLabel("Colour theme").selectOption("light");
  const native = (await rpc("appearance.accent")).result;
  await expect(page.locator("html")).toHaveAttribute("data-theme", "light");
  await expect
    .poll(() =>
      page
        .locator("html")
        .evaluate((element) =>
          getComputedStyle(element).getPropertyValue("--system-blue").trim(),
        ),
    )
    .toBe(native.palettes.light.blue);
  await page.getByRole("button", { name: "Back", exact: true }).click();
  await expect(page.locator(".browse-release-cover img")).toBeVisible();
  await page.screenshot({
    path: "/private/tmp/tibrary-browse-release-light.png",
  });
});

test("browse account changes invalidate the in-memory home cache", async ({
  page,
}) => {
  await page.addInitScript(() => {
    let id = 0;
    const callbacks = new Map<number, (event: any) => void>();
    const listeners = new Map<number, string>();
    (window as any).__TAURI_INTERNALS__ = {
      transformCallback: (callback: (event: any) => void) => {
        callbacks.set(++id, callback);
        return id;
      },
      invoke: async (command: string, args: any) => {
        if (command === "plugin:event|listen") {
          listeners.set(args.handler, args.event);
          return args.handler;
        }
        if (command === "plugin:event|unlisten") callbacks.delete(args.eventId);
      },
    };
    (window as any).__TAURI_EVENT_PLUGIN_INTERNALS__ = {
      unregisterListener: (_event: string, eventId: number) =>
        listeners.delete(eventId),
    };
    (window as any).emitBrowserBackendEvent = (payload: any) => {
      for (const [handler, event] of listeners)
        if (event === "backend-event")
          callbacks.get(handler)?.({ event, id: handler, payload });
    };
  });
  let connected = true;
  override = async (request) => {
    if (request.method === "browse.home")
      return { result: { ...home(), connected } };
    if (request.method !== "state") return undefined;
    const response = await rpc(request.method, request.args);
    response.result.connections = {
      ...response.result.connections,
      account: connected,
      tidal: { user_id: connected ? "fixture-account" : "" },
    };
    return response;
  };
  await browse(page);
  const calls = requests.filter(
    (request) => request.method === "browse.home",
  ).length;
  connected = false;
  await page.evaluate(() =>
    (window as any).emitBrowserBackendEvent({ event: "authentication" }),
  );
  await expect(
    page.getByRole("button", { name: "Connect account", exact: true }),
  ).toBeVisible();
  expect(
    requests.filter((request) => request.method === "browse.home").length,
  ).toBeGreaterThan(calls);
});
