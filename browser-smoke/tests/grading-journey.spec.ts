import { test, expect, type Page } from '@playwright/test';

// SpeedGrader grading journey (issue #108). Credentials + sentinel ids are
// defined LOCALLY (grading-teacher / grading-student, course 996101,
// assignment 996031, submission 996041, rubric criteria 996061/996062,
// ratings 996071..996074) so this spec never touches the base seed.sql
// helpers — seed-grading.sql runs BEFORE seed.sql in glob order and is fully
// self-sufficient.
const TEACHER_EMAIL = 'grading-teacher@test.local';
const STUDENT_EMAIL = 'grading-student@test.local';
const PASSWORD = 'SmokeTest-123!';

const COURSE_ID = 996101;
const ASSIGNMENT_ID = 996031;
const CRITERION_THESIS = 996061;
const CRITERION_EVIDENCE = 996062;
const RATING_FULL_THESIS = 996071; // 5 pts
const RATING_SOME_EVIDENCE = 996074; // 2 pts
const STUDENT_NAME = 'Grading Smoke Student';

async function login(page: Page, email: string): Promise<void> {
  await page.goto('/dashboard');
  await expect(page).toHaveURL(/\/login\?next=/);
  await expect(page.locator('#login_form')).toBeVisible();
  await page.getByLabel('Email').fill(email);
  await page.getByLabel('Password').fill(PASSWORD);
  await page.getByRole('button', { name: 'Log In' }).click();
  await expect(page).toHaveURL(/\/dashboard$/);
}

test.describe('grading journey (#108)', () => {
  // The teacher grades the seeded submission; the student-blocked check
  // runs after. Static HTML, but keep the journey ordered for clarity.
  test.describe.configure({ mode: 'serial' });

  test('teacher grades the submission end-to-end (score, comment, rubric)', async ({ page }) => {
    await login(page, TEACHER_EMAIL);

    // Open the SpeedGrader queue (default assignment = the only one).
    await page.goto(`/courses/${COURSE_ID}/gradebook/speed_grader`);
    const speedgrader = page.locator('#speedgrader');
    await expect(speedgrader).toBeVisible();

    // Left nav: the student queue lists the enrolled student.
    const row = page.locator(
      `.speedgrader-student-row[data-user-id] a:has-text("${STUDENT_NAME}")`,
    );
    await expect(row).toBeVisible();
    await expect(row.locator('.pill-submitted')).toHaveText('Submitted');

    // Main panel: the submission text + attempt info + current score dash.
    await expect(page.locator('#speedgrader_submission_body')).toContainText(
      'My smoke grading essay text',
    );
    await expect(page.locator('#speedgrader_current_score')).toHaveText('–');
    await expect(page.locator('#speedgrader_assignment_title')).toContainText(
      'Smoke Grading Essay',
    );

    // Rubric panel renders with the two criteria.
    await expect(page.locator('#speedgrader_rubric')).toBeVisible();
    await expect(page.locator(`#criterion_${CRITERION_THESIS}`)).toBeVisible();
    await expect(page.locator(`#criterion_${CRITERION_EVIDENCE}`)).toBeVisible();

    // The grading form: score + comment + rubric picks in ONE submit.
    await page.locator('#speedgrader_score').fill('8.5');
    await page.locator('#speedgrader_comment').fill('Nice work on the thesis.');
    await page.locator(`#criterion_${CRITERION_THESIS}`).selectOption(String(RATING_FULL_THESIS));
    await page
      .locator(`#criterion_${CRITERION_EVIDENCE}`)
      .selectOption(String(RATING_SOME_EVIDENCE));
    await page.locator('#speedgrader_submit').click();

    // PRG back to the same student + assignment with the success notice.
    await expect(page).toHaveURL(
      new RegExp(
        `/courses/${COURSE_ID}/gradebook/speed_grader\\?assignment_id=${ASSIGNMENT_ID}.*&graded=1$`,
      ),
    );
    await expect(page.locator('.ic-flash.success')).toContainText('Grade saved.');

    // Score persists after reload (fresh GET, no JS state).
    await page.reload();
    await expect(page.locator('#speedgrader_current_score')).toHaveText('8.5 / 10');
    await expect(page.locator('#speedgrader_score')).toHaveValue('8.5');
    const gradedRow = page.locator(
      `.speedgrader-student-row[data-user-id] a:has-text("${STUDENT_NAME}")`,
    );
    await expect(gradedRow.locator('.pill-graded')).toHaveText('Graded');

    // Comment visible in the feed.
    await expect(page.locator('#speedgrader_comments')).toContainText(
      'Nice work on the thesis.',
    );

    // Rubric picks persist: the selects keep the chosen ratings.
    await expect
      .poll(async () =>
        page.locator(`#criterion_${CRITERION_THESIS}`).inputValue(),
      )
      .toBe(String(RATING_FULL_THESIS));
    await expect
      .poll(async () =>
        page.locator(`#criterion_${CRITERION_EVIDENCE}`).inputValue(),
      )
      .toBe(String(RATING_SOME_EVIDENCE));
  });

  test('student is blocked from the grading queue (403)', async ({ page }) => {
    await login(page, STUDENT_EMAIL);

    const response = await page.goto(`/courses/${COURSE_ID}/gradebook/speed_grader`);
    expect(response?.status()).toBe(403);
    await expect(page.locator('#speedgrader')).toHaveCount(0);
  });
});
