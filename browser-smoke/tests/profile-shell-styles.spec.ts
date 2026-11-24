import { expect, test } from '@playwright/test';
import { SMOKE_PASSWORD, TEACHER_EMAIL, login } from './helpers';

/**
 * The `/profile` shell: the document axum builds, the Canvas stylesheets it names, and
 * the session cookie the bundle authenticates with.
 *
 * This is the only place the three halves of the frontend swap are proved together. A
 * Rust test can assert the HTML contains a `<link>`; it cannot assert that the sheet
 * behind the link paints an 84 px rail. A vitest run can assert `api.ts` sets a header;
 * it cannot assert the browser actually had a cookie to send.
 *
 * Every measurement here is Canvas' own, taken from `static/canvas/normal_contrast/`
 * (built from `canvas-src/app/stylesheets`), so a failure means the real stylesheet set
 * is not reaching the page — not that a hand-ported value drifted.
 */

test('the shell loads Canvas stylesheets and the nav gets Canvas geometry', async ({ page }) => {
  await login(page, TEACHER_EMAIL, SMOKE_PASSWORD);
  await page.goto('/profile');

  // The server injected the per-request parts into the bundle's own document.
  await expect(page).toHaveTitle(/Profile/);
  await expect(page.locator('body')).toHaveClass(/primary-nav-expanded/);
  await expect(page.locator('#application.ic-app')).toHaveCount(1);

  const common = page.locator('link[data-canvas-bundle="common"]');
  await expect(common).toHaveCount(1);
  const href = await common.getAttribute('href');
  expect(href, 'the shell must emit a content-revisioned sheet URL').toMatch(
    /^\/canvas\/[0-9a-f]{8}\/normal_contrast\/common\.css$/
  );

  // Sheet order is the cascade: tokens (custom properties) before common (uses them).
  const bundles = await page.locator('link[data-canvas-bundle]').evaluateAll((links) =>
    links.map((link) => link.getAttribute('data-canvas-bundle'))
  );
  expect(bundles.indexOf('tokens')).toBeLessThan(bundles.indexOf('common'));
  expect(bundles.indexOf('common')).toBeLessThan(bundles.indexOf('profile_show'));

  // A `<link>` that 404s still appears in the DOM, so assert the sheet is in
  // document.styleSheets — and that a Canvas rule is the one being applied.
  const loaded = await page.evaluate(() =>
    Array.from(document.styleSheets).filter((sheet) => sheet.href?.includes('/canvas/')).length
  );
  expect(loaded, 'every Canvas sheet must parse, not just be requested').toBe(bundles.length);

  const rail = page.locator('header#header.ic-app-header');
  await expect(rail).toBeVisible();
  expect(
    await rail.evaluate((header) => getComputedStyle(header).width),
    'common.css gives the global nav an 84px rail only when body carries primary-nav-expanded'
  ).toBe('84px');
  expect(
    await page
      .locator('#wrapper.ic-Layout-wrapper')
      .evaluate((el) => getComputedStyle(el).marginLeft),
    'and shifts the content column by the same 84px'
  ).toBe('84px');

  // Typography, i.e. the part a hand-ported sheet always got wrong.
  const family = await page.evaluate(() => getComputedStyle(document.body).fontFamily);
  expect(family, 'Lato Extended must be the first family, as in Canvas common.css').toContain(
    'Lato Extended'
  );
  expect(
    await page.evaluate(() => getComputedStyle(document.body).backgroundColor)
  ).toBe('rgb(255, 255, 255)');
});

test('the bundle talks to /api/v1 with the session cookie, not a stored token', async ({
  page,
}) => {
  await login(page, TEACHER_EMAIL, SMOKE_PASSWORD);
  await page.goto('/profile');

  // HttpOnly means the page cannot read its own credential; the non-HttpOnly CSRF
  // cookie is the one `api.ts` echoes into a header.
  const readable = await page.evaluate(() => document.cookie);
  expect(readable).toContain('star_csrf=');
  expect(readable, 'star_session must stay out of document.cookie').not.toContain('star_session=');

  // No Authorization header, no token anywhere — the bridge turns the cookie into one.
  const me = await page.evaluate(async () => {
    const response = await fetch('/api/v1/auth/me', { credentials: 'same-origin' });
    return { status: response.status, body: await response.json().catch(() => null) };
  });
  expect(me.status, 'a cookie-only GET must authenticate').toBe(200);
  expect(me.body?.role).toBe('teacher');

  // A write without the CSRF header is refused, exactly as a cross-site one would be.
  const unguarded = await page.evaluate(async () => {
    const csrf = /(?:^|;\s*)star_csrf=([^;]+)/.exec(document.cookie)?.[1] ?? '';
    const response = await fetch('/api/v1/courses/990101/announcements', {
      method: 'POST',
      credentials: 'same-origin',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ title: 'no csrf header', message: 'x' }),
    });
    return { status: response.status, csrf: csrf.length };
  });
  expect(unguarded.csrf, 'the shell must have issued a CSRF cookie on this page').toBeGreaterThan(
    0
  );
  expect(unguarded.status, 'the bridge requires X-CSRF-Token on an unsafe method').toBe(403);
});

test('a Canvas sheet is immutable, and revalidating it costs no bytes', async ({
  page,
  request,
}) => {
  await login(page, TEACHER_EMAIL, SMOKE_PASSWORD);
  await page.goto('/profile');

  const href = await page.locator('link[data-canvas-bundle="common"]').getAttribute('href');
  expect(href).toMatch(/^\/canvas\/[0-9a-f]{8}\/normal_contrast\/common\.css$/);

  // Assets carry no session, so `request` (no cookies) is the honest client here.
  const first = await request.get(href!);
  expect(first.status()).toBe(200);
  expect(first.headers()['cache-control']).toBe('public, max-age=31536000, immutable');
  expect(first.headers()['content-type']).toBe('text/css; charset=utf-8');

  const again = await request.get(href!, { headers: { 'if-none-match': first.headers()['etag'] } });
  expect(again.status(), 'a repeat view must not resend 400KB of CSS').toBe(304);

  // The unversioned form the SPA uses mid-navigation must still resolve, but it cannot
  // be immutable: Canvas' own sheets name these paths, so they outlive any one build.
  const plain = await request.get('/canvas/normal_contrast/common.css');
  expect(plain.status()).toBe(200);
  expect(plain.headers()['cache-control']).toContain('must-revalidate');
});
