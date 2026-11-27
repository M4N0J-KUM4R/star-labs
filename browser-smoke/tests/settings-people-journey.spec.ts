import { test, expect } from '@playwright/test';

// Course settings + people journey (issue #73). Credentials + sentinel ids
// are defined LOCALLY (settings-teacher / settings-student /
// settings-extra, course 995101) so this spec never touches the base
// seed.sql helpers — seed-settings.sql runs BEFORE seed.sql in glob order
// and is fully self-sufficient.
const TEACHER_EMAIL = 'settings-teacher@test.local';
const STUDENT_EMAIL = 'settings-student@test.local';
const EXTRA_EMAIL = 'settings-extra@test.local';
const PASSWORD = 'SmokeTest-123!';

const COURSE_ID = 995101;
const STUDENT_ID = 995002;

async function login(page: import('@playwright/test').Page, email: string): Promise<void> {
  await page.goto('/dashboard');
  await expect(page).toHaveURL(/\/login\?next=/);
  await expect(page.locator('#login_form')).toBeVisible();
  await page.getByLabel('Email').fill(email);
  await page.getByLabel('Password').fill(PASSWORD);
  await page.getByRole('button', { name: 'Log In' }).click();
  await expect(page).toHaveURL(/\/dashboard$/);
}

test.describe('settings + people journey (#73)', () => {
  // The add-people assertion needs the roster after the settings save, and
  // the pages are static HTML — run the journey in order.
  test.describe.configure({ mode: 'serial' });

  test('teacher saves course settings and sees the success notice', async ({ page }) => {
    await login(page, TEACHER_EMAIL);

    await page.goto(`/courses/${COURSE_ID}/settings`);
    await expect(page.locator('#course_settings_form')).toBeVisible();
    await expect(page.locator('#course-status')).toContainText('Course is Published');

    // Save a new name + syllabus.
    await page.locator('#course_name').fill('Smoke Settings Course Renamed');
    await page.locator('#course_syllabus').fill('Updated syllabus body');
    await page.locator('#course_settings_submit').click();

    // PRG back to settings with the saved state rendered.
    await expect(page).toHaveURL(new RegExp(`/courses/${COURSE_ID}/settings\\?saved=1$`));
    await expect(page.locator('.ic-flash.success')).toContainText('Course settings saved.');
    await expect(page.locator('#course_settings_form')).toBeVisible();
    await expect(page.locator('#course_name')).toHaveValue('Smoke Settings Course Renamed');
    await expect(page.locator('#syllabus_summary')).toContainText('Updated syllabus body');
  });

  test('teacher adds a person and the roster row appears', async ({ page }) => {
    await login(page, TEACHER_EMAIL);

    await page.goto(`/courses/${COURSE_ID}/users`);
    await expect(page.locator('#roster_table')).toBeVisible();
    await expect(page.locator('#add_people_link')).toBeVisible();
    await expect(page.locator(`#user_${STUDENT_ID}`)).toContainText('Settings Smoke Student');
    await expect(page.locator(`#user_${STUDENT_ID}`)).toContainText('Student');

    // Add the extra user.
    await page.locator('#add_people_link').click();
    await expect(page).toHaveURL(new RegExp(`/courses/${COURSE_ID}/users/add$`));
    await page.locator('#add_people_login').fill(EXTRA_EMAIL);
    await page.locator('#add_people_submit').click();

    // PRG back to the people page with the new roster row.
    await expect(page).toHaveURL(new RegExp(`/courses/${COURSE_ID}/users\\?added=1$`));
    await expect(page.locator('.ic-flash.success')).toContainText('Person added');
    await expect(page.locator('#roster_table')).toContainText('Settings Smoke Extra');
  });

  test('student is blocked from the settings and add-people surfaces (403)', async ({ page }) => {
    await login(page, STUDENT_EMAIL);

    // Settings page: 403 page, no settings form.
    const settings = await page.goto(`/courses/${COURSE_ID}/settings`);
    expect(settings?.status()).toBe(403);
    await expect(page.locator('#course_settings_form')).toHaveCount(0);

    // The people page itself renders for the student, but with no teacher
    // controls.
    await page.goto(`/courses/${COURSE_ID}/users`);
    await expect(page.locator('#roster_table')).toBeVisible();
    await expect(page.locator('#add_people_link')).toHaveCount(0);

    // Add-people form: 403 page.
    const add = await page.goto(`/courses/${COURSE_ID}/users/add`);
    expect(add?.status()).toBe(403);
    await expect(page.locator('#add_people_form')).toHaveCount(0);
  });
});
