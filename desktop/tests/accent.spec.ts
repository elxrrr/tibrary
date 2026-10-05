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
    <div class="context-menu" style="left:400px;top:200px"><button id="menu">Open on web</button></div>`);

  const background = (selector: string) => page.locator(selector).evaluate(element => getComputedStyle(element).backgroundColor);
  for (const theme of ["light", "dark"]) {
    const selectedColours = new Set<string>();
    const hoverColours = new Set<string>();
    for (const highlight of ["system", "orange", "graphite"]) {
      await page.evaluate(({ theme, highlight }) => {
        document.documentElement.dataset.theme = theme;
        document.documentElement.dataset.highlight = highlight;
        document.documentElement.style.setProperty("--system-accent", "#ff2d55");
      }, { theme, highlight });
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
      expect(await page.locator("#filter").evaluate(element => getComputedStyle(element).color))
        .toBe(await page.locator("#sort").evaluate(element => getComputedStyle(element).color));
      selectedColours.add(selected);
      hoverColours.add(hovered);
    }
    expect(selectedColours.size).toBe(3);
    expect(hoverColours.size).toBe(3);
  }
});
