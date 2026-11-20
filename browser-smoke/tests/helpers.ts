import { Page, expect } from '@playwright/test';

// Seeded credentials and sentinel ids — must match browser-smoke/fixtures/seed.sql.
export const TEACHER_EMAIL = 'smoke-teacher@test.local';
export const STUDENT_EMAIL = 'smoke-student@test.local';
export const SMOKE_PASSWORD = 'SmokeTest-123!';

export const COURSE_FRONT_PAGE_ID = 990101; // "Smoke Course Alpha" — has a front page
export const COURSE_MODULES_ID = 990102; // "Smoke Course Beta" — no front page, one module
export const COURSE_FRONT_PAGE_NAME = 'Smoke Course Alpha';
export const COURSE_MODULES_NAME = 'Smoke Course Beta';
export const FRONT_PAGE_HEADING = 'Welcome to Smoke Course';
export const MODULE_NAME = 'Smoke Module One';

/**
 * Log in through the real form flow. Entering via /dashboard (instead of
 * /login directly) makes the redirect chain populate the hidden `next`
 * field, so a successful POST always lands back on /dashboard.
 */
export async function login(page: Page, email: string, password: string): Promise<void> {
  await page.goto('/dashboard');
  await expect(page).toHaveURL(/\/login\?next=/);
  await expect(page.locator('#login_form')).toBeVisible();

  await page.getByLabel('Email').fill(email);
  await page.getByLabel('Password').fill(password);
  await page.getByRole('button', { name: 'Log In' }).click();

  await expect(page).toHaveURL(/\/dashboard$/);
}
