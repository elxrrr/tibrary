import { test, expect } from "@playwright/test";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

test("highlight colour reaches navigation and interaction surfaces in Light and Dark", async ({ page }) => {
  await page.emulateMedia({ reducedMotion: "reduce" });
  await page.setContent(`<style>${readFileSync(resolve("src/style.css"), "utf8")}</style>
    <aside>
      <button class="nav-item current" id="current">Overview</button>
      <button class="nav-item" id="hover">Missing releases</button>
      <div class="nav-heading"><button>Toggle</button><button class="current" id="heading">Complete library</button></div>
    </aside>
    <button class="primary" id="primary">Queue selected</button>
    <button disabled id="disabled">Checking</button>
    <table><thead><tr><th aria-sort="ascending"><div class="table-header-actions"><button id="sort">Release</button><button class="column-filter-button active" id="filter">Filter</button></div></th></tr></thead>
      <tbody><tr class="child selected" id="selected"><td>Track</td></tr></tbody></table>
    <div class="context-menu" style="left:400px;top:200px"><button id="menu">Open on web</button></div>
    <span class="badge mqa-signal" id="signal">MQA signal</span>
    <span class="badge no-signal-found" id="clear">No signal found</span>
    <span id="selection">Selectable track details</span>
    <span id="native-selection" style="background:var(--system-selection);color:var(--system-selection-text)"></span>
    <div class="header-context-menu" style="position:absolute;left:400px;top:400px">
      <button role="menuitemcheckbox" aria-checked="true"><svg id="header-check" width="14" height="14"><path d="M2 7l3 3 7-7"/></svg>Recommended</button>
    </div>
    <svg class="action-menu-check" id="action-check" width="14" height="14"><path d="M2 7l3 3 7-7"/></svg>
    <svg class="scan-scope-check" id="scan-check" width="14" height="14"><path d="M2 7l3 3 7-7"/></svg>`);

  const background = (selector: string) => page.locator(selector).evaluate(element => getComputedStyle(element).backgroundColor);
  const colour = (selector: string) => page.locator(selector).evaluate(element => getComputedStyle(element).color);
  for (const theme of ["light", "dark"]) {
    const selectedColours = new Set<string>();
    const hoverColours = new Set<string>();
    let semanticColours: string[] | undefined;
    for (const nativeAccent of ["#ff2d55", theme === "dark" ? "#f1a347" : "#ed832b", theme === "dark" ? "#a4aeba" : "#77818b"]) {
      await page.evaluate(({ theme, nativeAccent }) => {
        document.documentElement.dataset.theme = theme;
        // The app receives these dynamic colours from macOS. This stylesheet
        // fixture supplies native accent changes without a running app.
        document.documentElement.style.setProperty("--system-accent", nativeAccent);
        document.documentElement.style.setProperty("--system-orange", theme === "dark" ? "#f1a347" : "#ed832b");
        document.documentElement.style.setProperty("--system-graphite", theme === "dark" ? "#a4aeba" : "#77818b");
        const native = theme === "dark"
          ? { red: "#ec746c", green: "#7bc18a", blue: "#4791ec", purple: "#c779d8", pink: "#ec77ac", yellow: "#eed557", selection: "#385a7e", "selection-text": "#f5f8ff" }
          : { red: "#c84c44", green: "#428d57", blue: "#236ecc", purple: "#ad49bc", pink: "#c94979", yellow: "#d3ad1c", selection: "#bdd7f2", "selection-text": "#172f48" };
        for (const [name, value] of Object.entries(native)) document.documentElement.style.setProperty(`--system-${name}`, value);
      }, { theme, nativeAccent });
      const selected = await background("#current");
      expect(await background("#heading")).toBe(selected);
      expect(await background("#primary")).toBe(selected);
      expect(await background("#selected")).toBe(selected);
      await page.locator("#menu").hover();
      expect(await background("#menu")).toBe(selected);
      await page.locator("#hover").hover();
      const hovered = await background("#hover");
      expect(hovered).not.toBe(selected);
      await page.locator("#current").hover();
      expect(await background("#current")).toBe(selected);
      const disabled = await background("#disabled");
      await page.locator("#disabled").hover();
      expect(await background("#disabled")).toBe(disabled);
      expect(await colour("#filter")).toBe(await colour("#sort"));
      const statusColours = [await colour("#signal"), await background("#signal"), await colour("#clear"), await background("#clear")];
      expect(statusColours[0]).not.toBe(statusColours[2]);
      if (semanticColours) expect(statusColours).toEqual(semanticColours);
      else semanticColours = statusColours;
      const textSelection = await page.locator("#selection").evaluate(element => {
        const selection = getComputedStyle(element, "::selection");
        return { background: selection.backgroundColor, colour: selection.color };
      });
      expect(textSelection.background).toBe(await background("#native-selection"));
      expect(textSelection.colour).toBe(await colour("#native-selection"));
      expect(await colour("#header-check")).toBe(await colour("#action-check"));
      expect(await colour("#header-check")).toBe(await colour("#scan-check"));
      selectedColours.add(selected);
      hoverColours.add(hovered);
    }
    expect(selectedColours.size).toBe(3);
    expect(hoverColours.size).toBe(3);
  }
});
