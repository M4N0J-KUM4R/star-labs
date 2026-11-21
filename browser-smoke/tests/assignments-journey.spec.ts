import { test, expect, type Page } from '@playwright/test';

// Journey 4 (issue #71): assignments + gradebook.
//
// This spec is deliberately self-contained: the sentinel ids and credentials
// come from browser-smoke/fixtures/seed-assignments.sql (NOT helpers.ts /
// seed.sql), because that fragment owns the assignments/gradebook fixture
// (own users, course and enrollments — it must not depend on seed.sql's rows,
// since the workflow applies seed*.sql in glob order and seed-assignments.sql
// runs FIRST). The login flow is the same real form flow helpers.ts drives.

const ASSIGNMENTS_TEACHER_EMAIL = 'assignments-teacher@test.local';
const ASSIGNMENTS_STUDENT_EMAIL = 'assignments-student@test.local';
const SMOKE_PASSWORD = 'SmokeTest-123!';

const COURSE_ID = 992101; // "Smoke Assignments Course"

// Sentinels from browser-smoke/fixtures/seed-assignments.sql (9920xx).
const GROUP_ESSAYS = 'Smoke Essays';
const GROUP_QUIZZES = 'Smoke Quizzes';
const ESSAY_ONE = 'Smoke Essay One';
const ESSAY_TWO = 'Smoke Essay Two';
const DRAFT_QUIZ = 'Smoke Draft Quiz';
const STUDENT_NAME = 'Assignments Student';

async function login(page: Page, email: string): Promise<void> {
  await page.goto('/dashboard');
  await expect(page).toHaveURL(/\/login\?next=/);
  await expect(page.locator('#login_form')).toBeVisible();

  await page.getByLabel('Email').fill(email);
  await page.getByLabel('Password').fill(SMOKE_PASSWORD);
  await page.getByRole('button', { name: 'Log In' }).click();

  await expect(page).toHaveURL(/\/dashboard$/);
}

// The teacher journey creates the assignment the gradebook journey then
// sees, and the student journey submits against the seeded rows, so the
// tests must run in order (fullyParallel would race them).
test.describe.configure({ mode: 'serial' });

// Journey (issue #71): teacher assignments index -> assignment show ->
// create via the new-assignment form -> gradebook grid + CSV; student
// grades page -> text-entry submit flow.
test.describe('journey 4: assignments + gradebook', () => {
  test('teacher: index groups, unpublished style, create assignment, gradebook grid', async ({ page }) => {
    await login(page, ASSIGNMENTS_TEACHER_EMAIL);

    // Assignments index: group buckets + rows + unpublished styling.
    await page.goto(`/courses/${COURSE_ID}/assignments`);
    const index = page.locator('#assignments_index');
    await expect(index).toBeVisible();
    await expect(page.locator('.ig-header-title', { hasText: GROUP_ESSAYS })).toBeVisible();
    await expect(page.locator('.ig-header-title', { hasText: GROUP_QUIZZES })).toBeVisible();
    await expect(page.locator('.ig-title', { hasText: ESSAY_ONE })).toBeVisible();
    await expect(page.locator('.ig-title', { hasText: ESSAY_TWO })).toBeVisible();
    // Unpublished styling on the draft quiz.
    const draftRow = page.locator('.ig-row-unpublished', { hasText: DRAFT_QUIZ });
    await expect(draftRow).toBeVisible();
    await expect(draftRow.locator('.publish-state-unpublished')).toHaveText('Unpublished');
    // Teacher chrome.
    await expect(page.locator('#add_assignment')).toBeVisible();
    await expect(page.locator('#open_gradebook')).toBeVisible();

    // Open the assignment show page.
    await page.locator('.ig-title', { hasText: ESSAY_ONE }).click();
    await expect(page).toHaveURL(
      new RegExp(`/courses/${COURSE_ID}/assignments/\\d+$`),
    );
    await expect(
      page.getByRole('heading', { name: ESSAY_ONE, exact: true }),
    ).toBeVisible();
    // Description rendered safe (tags stripped, text kept).
    await expect(page.locator('#assignment_description')).toContainText(
      'Write about the smoke journey.',
    );
    await expect(
      page.locator('#assignment_description script'),
    ).toHaveCount(0);
    // Teacher read-only submissions table (grading UI is #108).
    await expect(page.locator('#submissions_table')).toBeVisible();
    await expect(
      page.locator('#submissions_table .submission-row'),
    ).toHaveCount(1);

    // Create a new assignment through the form.
    await page.goto(`/courses/${COURSE_ID}/assignments/new`);
    await expect(page.locator('#assignment_edit_form')).toBeVisible();
    await page.locator('#assignment_name').fill('Smoke Created Essay');
    await page.locator('#assignment_description').fill('Created by the browser-smoke journey.');
    await page.locator('#assignment_points').fill('15');
    await page.locator('#assignment_due_date').fill('2026-12-15');
    await page.locator('#assignment_published').check();
    await page.locator('#assignment_form_submit').click();
    await expect(page).toHaveURL(
      new RegExp(`/courses/${COURSE_ID}/assignments$`),
    );

    // The new row appears in the list.
    const created = page.locator('.ig-row', { hasText: 'Smoke Created Essay' });
    await expect(created).toBeVisible();
    await expect(created.locator('.publish-state-published')).toHaveText('Published');

    // Gradebook grid renders: teacher + student rows, assignment columns.
    await page.goto(`/courses/${COURSE_ID}/gradebook`);
    await expect(page.locator('#gradebook_grid')).toBeVisible();
    await expect(
      page.locator('#gradebook_grid .assignment-header', { hasText: ESSAY_ONE }),
    ).toBeVisible();
    await expect(
      page.locator('#gradebook_grid .assignment-header', { hasText: 'Smoke Created Essay' }),
    ).toBeVisible();
    // One row per enrollment (teacher 992001 + student 992002).
    expect(await page.locator('#gradebook_grid .gradebook-row').count()).toBe(2);
    // The student's Essay One cell shows the graded 8.
    const studentRow = page.locator('.gradebook-row', { hasText: STUDENT_NAME });
    await expect(studentRow.locator('.gradebook-score', { hasText: '8' })).toBeVisible();
  });

  test('teacher: CSV export serves text/csv with header and data row', async ({ page }) => {
    await login(page, ASSIGNMENTS_TEACHER_EMAIL);

    // page.request inherits the browser session cookies.
    const resp = await page.request.get(
      `/courses/${COURSE_ID}/gradebook.csv`,
    );
    expect(resp.status()).toBe(200);
    const contentType = resp.headers()['content-type'] ?? '';
    expect(contentType).toContain('text/csv');
    const csv = await resp.text();
    const lines = csv.trim().split('\n');
    expect(lines[0]).toContain('Student');
    expect(lines[0]).toContain(ESSAY_ONE);
    const studentRow = lines.find((l) => l.includes(STUDENT_NAME));
    expect(studentRow).toBeDefined();
    expect(studentRow!).toContain('8');
  });

  test('student: grades page shows score; text-entry submit flow', async ({ page }) => {
    await login(page, ASSIGNMENTS_STUDENT_EMAIL);

    // Grades page: own score + total for the seeded 8/10.
    await page.goto(`/courses/${COURSE_ID}/grades`);
    await expect(page.locator('#grades_summary')).toBeVisible();
    const essayOneRow = page.locator('#grades_summary .student_assignment', {
      hasText: ESSAY_ONE,
    });
    await expect(essayOneRow).toBeVisible();
    await expect(essayOneRow.locator('td.assignment_score')).toContainText('8 / 10');
    await expect(page.locator('#final_grade_percent')).toContainText('80%');
    // Student sees no gradebook link.
    await expect(page.locator('#view_gradebook_link')).toHaveCount(0);

    // Text-entry submit on the un-submitted Essay Two.
    await page.goto(`/courses/${COURSE_ID}/assignments`);
    await page.locator('.ig-title', { hasText: ESSAY_TWO }).click();
    await expect(page.locator('#assignment_show')).toBeVisible();
    await expect(page.locator('.submission-pill')).toContainText('Not Submitted');
    await expect(page.locator('#submit_assignment')).toBeVisible();

    await page.locator('#submission_body').fill('My text entry submission.');
    await page.locator('#submit_assignment_button').click();
    await expect(page).toHaveURL(/\?submitted=1$/);

    // The pill flips to Submitted.
    await expect(page.locator('.submission-pill')).toContainText('Submitted');

    // The student does not see the teacher's submissions table.
    await expect(page.locator('#submissions_table')).toHaveCount(0);
    // The draft quiz never appears for the student.
    await expect(page.getByText(DRAFT_QUIZ)).toHaveCount(0);
  });

  test('student: gradebook grid and CSV are teacher-only (403)', async ({ page }) => {
    await login(page, ASSIGNMENTS_STUDENT_EMAIL);

    const grid = await page.goto(`/courses/${COURSE_ID}/gradebook`);
    expect(grid?.status()).toBe(403);

    const csv = await page.goto(`/courses/${COURSE_ID}/gradebook.csv`);
    expect(csv?.status()).toBe(403);
  });
});
