import { test, expect } from '@playwright/test';
import {
  login,
  TEACHER_EMAIL,
  SMOKE_PASSWORD,
  COURSE_FRONT_PAGE_ID,
} from './helpers';

// Journey 1 (spec §4): login -> dashboard.
test.describe('journey 1: login -> dashboard', () => {
  test('logged-out / and /dashboard redirect to /login with a next param', async ({ page }) => {
    await page.goto('/');
    // GET / -> /dashboard -> (logged out) /login?next=%2Fdashboard
    await expect(page).toHaveURL(/\/login\?next=/);
    await expect(page.locator('#login_form')).toBeVisible();

    await page.goto('/dashboard');
    await expect(page).toHaveURL(/\/login\?next=/);
    await expect(page.getByRole('button', { name: 'Log In' })).toBeVisible();
  });

  test('teacher logs in and lands on the dashboard with course cards and ENV', async ({ page }) => {
    await login(page, TEACHER_EMAIL, SMOKE_PASSWORD);

    // Global nav is present on the authenticated shell.
    await expect(page.locator('#global_nav_dashboard_link')).toBeVisible();

    // At least one course card for an enrolled course, linking /courses/{id}. The card
    // is identified by the link the cards API put in its payload — Canvas' component has
    // no data-course-id attribute (ui/shared/dashboard-card/react/DashboardCard.tsx:311-316
    // gives only class, style, aria-label and data-testid="draggable-card").
    const cards = page.locator('.ic-DashboardCard');
    // The grid arrives from `GET /api/v1/dashboard/dashboard_cards` after boot, so wait
    // for the first card instead of reading a count that is still 0 at this instant.
    await expect(cards.first()).toBeVisible();
    expect(await cards.count()).toBeGreaterThanOrEqual(1);

    const card = page.locator(
      `.ic-DashboardCard:has(a.ic-DashboardCard__link[href="/courses/${COURSE_FRONT_PAGE_ID}"])`,
    );
    await expect(card).toBeVisible();
    await expect(card.locator('a.ic-DashboardCard__link')).toHaveAttribute(
      'href',
      `/courses/${COURSE_FRONT_PAGE_ID}`,
    );

    // js_env contract: the inline window.ENV block identifies the user.
    const userId = await page.evaluate<number | undefined>(
      () => (window as { ENV?: { current_user_id?: number } }).ENV?.current_user_id,
    );
    expect(userId).toBeDefined();
    expect(Number(userId)).toBeGreaterThan(0);
  });
});
