import { defineConfig } from "@playwright/test";
export default defineConfig({
  testDir: "tests",
  workers: 1,
  use: {
    baseURL: "http://127.0.0.1:1420",
    viewport: { width: 1600, height: 1000 },
  },
  webServer: {
    command: "VITE_TEST_BRIDGE=1 npm run dev",
    url: "http://127.0.0.1:1420",
    reuseExistingServer: false,
  },
  projects: [{ name: "webkit", use: { browserName: "webkit" } }],
});
