import { defineConfig, devices } from '@playwright/test';

// The workflow exports STAR_BASE_URL (see .github/workflows/browser-smoke.yml,
// which boots star-http on STAR_PORT=18089). Locally the same default is used
// so `npx playwright test` works against `cargo run -p star-http` unchanged.
const baseURL = process.env.STAR_BASE_URL ?? 'http://localhost:18089';

export default defineConfig({
  testDir: './tests',
  fullyParallel: true,
  // No webServer: CI starts the real star-http binary itself (after the
  // postgres service is up) and waits on /health before seeding + testing.
  retries: 0,
  workers: process.env.CI ? 2 : undefined,
  reporter: [['list'], ['html', { open: 'never' }]],
  use: {
    baseURL,
    trace: 'on-first-retry',
    screenshot: 'only-on-failure',
  },
  projects: [
    {
      name: 'chromium',
      use: { ...devices['Desktop Chrome'] },
    },
  ],
});
