import { test, expect } from '@playwright/test';
import {
  login,
  TEACHER_EMAIL,
  STUDENT_EMAIL,
  SMOKE_PASSWORD,
  COURSE_FRONT_PAGE_ID,
  COURSE_FRONT_PAGE_NAME,
} from './helpers';

// Journey 3 (spec §4): access control + logout.
test.describe('journey 3: access control + logout', () => {
  test('logged-out /dashboard redirects to /login', async ({ page }) => {
    await page.goto('/dashboard');
    await expect(page).toHaveURL(/\/login\?next=/);
    await expect(page.locator('#login_form')).toBeVisible();
  });

  test('unenrolled student gets no course data', async ({ page }) => {
    await login(page, STUDENT_EMAIL, SMOKE_PASSWORD);

    const response = await page.goto(`/courses/${COURSE_FRONT_PAGE_ID}`);
    // Same 404-before-403 enrolled-or-admin gate as /api/v1/courses/:id.
    expect([403, 404]).toContain(response?.status());
    await expect(page.locator('.ic-Error')).toBeVisible();
    // The course name must never leak to an unenrolled non-admin.
    await expect(page.getByText(COURSE_FRONT_PAGE_NAME)).toHaveCount(0);
  });

  test('teacher logout ends the session', async ({ page }) => {
    await login(page, TEACHER_EMAIL, SMOKE_PASSWORD);

    // The logout button lives inside form[action="/logout"] with the hidden
    // _csrf field, so a plain click submits a valid double-submit form.
    const logoutForm = page.locator('form[action="/logout"]');
    await expect(logoutForm).toBeVisible();
    await page.locator('#global_nav_logout_link').click();

    await expect(page).toHaveURL(/\/login\/?$/);

    // The old session no longer authorizes /dashboard.
    await page.goto('/dashboard');
    await expect(page).toHaveURL(/\/login\?next=/);
  });
});
