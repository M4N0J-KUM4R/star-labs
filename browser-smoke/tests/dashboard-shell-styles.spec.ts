import { expect, test } from '@playwright/test';
import {
  COURSE_FRONT_PAGE_ID,
  COURSE_MODULES_ID,
  SMOKE_PASSWORD,
  TEACHER_EMAIL,
  login,
} from './helpers';

/**
 * The `/dashboard` shell, measured.
 *
 * The page used to be an Askama document that painted its own cards from a query the
 * handler ran, then loaded a hand-written stylesheet on top. It is now what Canvas is:
 * a shell that boots the bundle, and a bundle that fetches
 * `GET /api/v1/dashboard/dashboard_cards` (`config/routes.rb`) — the same call
 * `app/views/shared/_dashboard_card.html.erb:43` prefetches — while every rule comes from
 * Canvas' own compiled `dashboard`/`dashboard_card` sheets.
 *
 * Each geometry number below is read out of `migration/ui/static/canvas/normal_contrast/`
 * (built from `canvas-src/app/stylesheets`) rather than guessed:
 *   .ic-DashboardCard               width 262px        (dashboard.css, cards.scss)
 *   .ic-DashboardCard__header_hero  height 146px       (dashboard_card.css)
 *   .ic-DashboardCard__action-container height 2.5rem  (dashboard_card.css)
 *   .ic-DashboardCard__action       flex 0 0 25%       (dashboard_card.css)
 *   .ic-app-main-content__secondary width 288px        (common.css)
 *   .with-right-side #right-side-wrapper display block (common.css, base/_layout.scss:78)
 * All of them hold at the 1280px viewport: the only media queries touching these rules
 * are max-width 1024/991/767/620.
 */

test.describe('dashboard shell', () => {
  test('the Canvas dashboard sheets load in cascade order', async ({ page }) => {
    await login(page, TEACHER_EMAIL, SMOKE_PASSWORD);

    const bundles = await page.locator('link[data-canvas-bundle]').evaluateAll((links) =>
      links.map((link) => link.getAttribute('data-canvas-bundle')),
    );
    for (const name of ['tokens', 'common', 'dashboard', 'dashboard_card']) {
      expect(bundles, `${name} must be in the first-paint cascade`).toContain(name);
    }
    // tokens defines the custom properties common resolves, and the card sheet is the
    // most specific of the four.
    expect(bundles.indexOf('tokens')).toBeLessThan(bundles.indexOf('common'));
    expect(bundles.indexOf('common')).toBeLessThan(bundles.indexOf('dashboard'));
    expect(bundles.indexOf('dashboard')).toBeLessThan(bundles.indexOf('dashboard_card'));

    const href = await page
      .locator('link[data-canvas-bundle="dashboard_card"]')
      .getAttribute('href');
    expect(href, 'the shell emits a content-revisioned sheet URL').toMatch(
      /^\/canvas\/[0-9a-f]{8}\/normal_contrast\/dashboard_card\.css$/,
    );

    // A <link> that 404s still sits in the DOM, so prove each sheet parsed.
    const loaded = await page.evaluate(() =>
      Array.from(document.styleSheets).filter((sheet) => sheet.href?.includes('/canvas/')).length,
    );
    expect(loaded, 'every Canvas sheet must parse, not just be requested').toBe(bundles.length);

    // Typography is the part a hand-ported sheet got wrong, so re-prove it here.
    expect(await page.evaluate(() => getComputedStyle(document.body).fontFamily)).toContain(
      'Lato Extended',
    );
  });

  test('the cards Canvas paints its own geometry, over the session cookie', async ({ page }) => {
    // Register the listeners before the load so the boot-time fetches are captured.
    const cardsResponse = page.waitForResponse((response) =>
      response.url().includes('/api/v1/dashboard/dashboard_cards'),
    );
    const todoResponse = page.waitForResponse((response) =>
      response.url().includes('/api/v1/users/self/todo'),
    );

    await login(page, TEACHER_EMAIL, SMOKE_PASSWORD);

    const cards = await cardsResponse;
    expect(cards.status(), 'the dashboard fetches its cards over the cookie session').toBe(200);
    expect(
      (await cards.request()).headers()['authorization'],
      'the bundle holds no token; session_bridge turns the cookie into a bearer',
    ).toBeUndefined();
    const payload = await cards.json();
    expect(Array.isArray(payload)).toBe(true);
    expect(
      payload.map((card: { id: number }) => card.id).sort((a: number, b: number) => a - b),
    ).toEqual([COURSE_FRONT_PAGE_ID, COURSE_MODULES_ID]);

    // The To Do half: Canvas' sidebar is an HTML fragment (GET /dashboard-sidebar); this
    // port feeds the same rows from the JSON Canvas also serves, and a teacher with the
    // seeded courses has no todo items, so 200 + [] is the expected contract here.
    expect((await todoResponse).status()).toBe(200);

    const card = page.locator(
      `.ic-DashboardCard:has(a.ic-DashboardCard__link[href="/courses/${COURSE_FRONT_PAGE_ID}"])`,
    );
    await expect(card).toBeVisible();
    expect(
      await card.evaluate((el) => getComputedStyle(el).width),
      '.ic-DashboardCard{width:262px}',
    ).toBe('262px');

    const hero = card.locator('.ic-DashboardCard__header_hero');
    expect(await hero.evaluate((el) => getComputedStyle(el).height)).toBe('146px');

    const actions = card.locator('.ic-DashboardCard__action-container');
    const rem = await page.evaluate(() =>
      parseFloat(getComputedStyle(document.documentElement).fontSize),
    );
    expect(
      await actions.evaluate((el) => getComputedStyle(el).height),
      '.ic-DashboardCard__action-container{height:2.5rem}',
    ).toBe(`${rem * 2.5}px`);

    // flex: 0 0 25% of the card, from DashboardCardAction.jsx rendering the anchors as
    // direct children of the nav.
    const firstActionWidth = await actions
      .locator('.ic-DashboardCard__action')
      .first()
      .evaluate((el) => parseFloat(getComputedStyle(el).width));
    expect(firstActionWidth, 'each action takes a quarter of the 262px card').toBeCloseTo(
      262 * 0.25,
      0,
    );

    // The old page identified cards with an attribute Canvas never emits; if it comes
    // back, the payload href stopped being the thing the component links to.
    expect(await page.locator('.ic-DashboardCard[data-course-id]').count()).toBe(0);
  });

  test('the right side is reserved only on the page that provides it', async ({ page }) => {
    await login(page, TEACHER_EMAIL, SMOKE_PASSWORD);

    // user_dashboard.html.erb:84 `provide :right_side` is what makes Canvas' layout add
    // `with-right-side` (application.html.erb:94), and .with-right-side is the only thing
    // that un-hides #right-side-wrapper.
    await expect(page.locator('body')).toHaveClass(/with-right-side/);
    const wrapper = page.locator('#right-side-wrapper');
    expect(await wrapper.evaluate((el) => getComputedStyle(el).display)).toBe('block');
    expect(
      await wrapper.evaluate((el) => getComputedStyle(el).width),
      '.ic-app-main-content__secondary{width:288px}',
    ).toBe('288px');

    // Control: /profile gets no right side, so the same wrapper stays hidden there.
    await page.goto('/profile');
    await expect(page.locator('body')).not.toHaveClass(/with-right-side/);
    expect(
      await page.locator('#right-side-wrapper').evaluate((el) => getComputedStyle(el).display),
    ).toBe('none');
  });
});
