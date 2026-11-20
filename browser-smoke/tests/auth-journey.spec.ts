import { test, expect } from '@playwright/test';
import { SMOKE_PASSWORD } from './helpers';

// Auth journey (issue #70): registration, forgot password, wrong-then-right
// login, logout. Credentials/ids come from browser-smoke/fixtures/seed.sql
// (shared teacher/student) and seed-auth.sql (auth-smoke user).

const AUTH_USER_EMAIL = 'auth-smoke@test.local';

test.describe('auth journey: register, forgot password, login, logout', () => {
  test('register a fresh user via the form and land on /dashboard', async ({ page }) => {
    const email = `reg-${Date.now()}-${Math.random().toString(36).slice(2, 8)}@test.local`;

    await page.goto('/register');
    await expect(page.locator('#register_form')).toBeVisible();

    await page.getByLabel('Full Name').fill('Registered Smoke User');
    await page.getByLabel('Email', { exact: true }).fill(email);
    await page.getByLabel('Password', { exact: true }).fill('RegisteredPass123!');
    await page.getByLabel('Confirm Password').fill('RegisteredPass123!');
    await page.getByRole('button', { name: 'Register' }).click();

    // Auto-login after registration: straight to /dashboard with ENV.
    await expect(page).toHaveURL(/\/dashboard$/);
    const userId = await page.evaluate<number | undefined>(
      () => (window as { ENV?: { current_user_id?: number } }).ENV?.current_user_id,
    );
    expect(userId).toBeDefined();
    expect(Number(userId)).toBeGreaterThan(0);
  });

  test('forgot-password form for a known email shows the generic confirmation', async ({ page }) => {
    await page.goto('/forgot_password');
    await expect(page.locator('#forgot_password_form')).toBeVisible();

    await page.getByLabel('Email').fill(AUTH_USER_EMAIL);
    await page.getByRole('button', { name: 'Request Password' }).click();

    await expect(page.locator('#forgot_password_confirmation')).toBeVisible();
    await expect(page.locator('#forgot_password_confirmation')).toContainText(
      /if that account exists/i,
    );
  });

  test('wrong-then-right login and logout', async ({ page }) => {
    await page.goto('/login');
    await expect(page.locator('#login_form')).toBeVisible();

    // Wrong password: re-rendered form with the error, no redirect.
    await page.getByLabel('Email').fill(AUTH_USER_EMAIL);
    await page.getByLabel('Password').fill('Definitely-Wrong-1!');
    await page.getByRole('button', { name: 'Log In' }).click();
    await expect(page.locator('#login_error')).toContainText(/invalid email or password/i);
    await expect(page).toHaveURL(/\/login/);

    // Right password: lands on /dashboard.
    await page.getByLabel('Password').fill(SMOKE_PASSWORD);
    await page.getByRole('button', { name: 'Log In' }).click();
    await expect(page).toHaveURL(/\/dashboard$/);

    // Logout ends the session.
    await page.locator('#global_nav_logout_link').click();
    await expect(page).toHaveURL(/\/login\/?$/);
    await page.goto('/dashboard');
    await expect(page).toHaveURL(/\/login\?next=/);
  });
});
