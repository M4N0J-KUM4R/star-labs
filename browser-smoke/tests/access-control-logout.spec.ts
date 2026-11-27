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
    // `_method=delete` + `_csrf` fields — Canvas' own shape, `form_tag
    // logout_path, method: :delete` (app/views/login/logout_confirm.html.erb:39)
    // over `delete "logout" => "login#destroy"` (config/routes.rb:930). A plain
    // click submits the POST, the shell's method_override rewrites the verb, and
    // the double-submit pair still validates.
    const logoutForm = page.locator('form[action="/logout"]');
    await expect(logoutForm).toBeVisible();
    await expect(
      logoutForm.locator('input[name="_method"][value="delete"]'),
    ).toHaveCount(1);
    await page.locator('#global_nav_logout_link').click();

    await expect(page).toHaveURL(/\/login\/?$/);

    // The old session no longer authorizes /dashboard.
    await page.goto('/dashboard');
    await expect(page).toHaveURL(/\/login\?next=/);
  });
});
