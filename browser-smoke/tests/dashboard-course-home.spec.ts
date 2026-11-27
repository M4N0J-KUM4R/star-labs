import { test, expect } from '@playwright/test';
import {
  login,
  TEACHER_EMAIL,
  SMOKE_PASSWORD,
  COURSE_FRONT_PAGE_ID,
  COURSE_MODULES_ID,
  FRONT_PAGE_HEADING,
  MODULE_NAME,
} from './helpers';

// Fixed section-tab order from star-courses DEFAULT_COURSE_TABS; a teacher
// additionally gets the Settings tab pinned last (tabs API parity).
const EXPECTED_TAB_LABELS = [
  'Home',
  'Announcements',
  'Assignments',
  'Discussions',
  'Grades',
  'People',
  'Pages',
  'Files',
  'Syllabus',
  'Modules',
  'Outcomes',
  'Quizzes',
  'Settings',
];

// Journey 2 (spec §4): dashboard -> course home.
test.describe('journey 2: dashboard -> course home', () => {
  test('front-page course: tab order, front page body, course ENV', async ({ page }) => {
    await login(page, TEACHER_EMAIL, SMOKE_PASSWORD);

    // The card the dashboard bundle rendered from /api/v1/dashboard/dashboard_cards.
    await page
      .locator(
        `.ic-DashboardCard:has(a.ic-DashboardCard__link[href="/courses/${COURSE_FRONT_PAGE_ID}"]) a.ic-DashboardCard__link`,
      )
      .click();
    await expect(page).toHaveURL(new RegExp(`/courses/${COURSE_FRONT_PAGE_ID}/?$`));

    // Section tabs: all present, in the fixed API order.
    const tabs = page.locator('#section-tabs a');
    await expect(tabs).toHaveCount(EXPECTED_TAB_LABELS.length);
    // textContent (not innerText) so CSS-hidden tabs still count.
    const labels = (await tabs.allTextContents()).map((t) => t.trim());
    expect(labels).toEqual(EXPECTED_TAB_LABELS);

    // Home tab is the active one.
    await expect(page.locator('#home-link')).toHaveClass(/(^|\s)active(\s|$)/);

    // Front page body renders (wiki branch of course home).
    await expect(
      page.getByRole('heading', { name: FRONT_PAGE_HEADING }),
    ).toBeVisible();
    await expect(page.locator('#wiki_page_show .user_content')).toBeVisible();

    // js_env contract for the course context.
    const env = await page.evaluate(() =>
      (window as { ENV?: Record<string, unknown> }).ENV ?? {},
    );
    expect(env['context_asset_string']).toBe(`course_${COURSE_FRONT_PAGE_ID}`);
    expect(env['active_context_tab']).toBe('home');
  });

  test('modules course: module list renders when no front page exists', async ({ page }) => {
    await login(page, TEACHER_EMAIL, SMOKE_PASSWORD);
    await page.goto(`/courses/${COURSE_MODULES_ID}`);

    await expect(page.locator('#context_modules')).toBeVisible();
    await expect(page.locator('#context_modules .context_module')).toHaveCount(1);
    await expect(
      page.getByRole('heading', { name: MODULE_NAME, exact: true }),
    ).toBeVisible();
  });
});
