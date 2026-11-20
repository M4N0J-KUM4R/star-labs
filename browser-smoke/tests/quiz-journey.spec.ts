import { test, expect } from '@playwright/test';

// Quiz-taking journey (issue #109). Credentials + sentinel ids are defined
// LOCALLY (quiz-teacher / quiz-student, course 994101, quiz 994301) so this
// spec never touches the base seed.sql helpers — seed-quizzes.sql runs
// BEFORE seed.sql in glob order and is fully self-sufficient.
const TEACHER_EMAIL = 'quiz-teacher@test.local';
const STUDENT_EMAIL = 'quiz-student@test.local';
const PASSWORD = 'SmokeTest-123!';

const COURSE_ID = 994101;
const QUIZ_ID = 994301;
const MC_ID = 994401;
const TF_ID = 994402;
const ESSAY_ID = 994403;

async function login(page: import('@playwright/test').Page, email: string): Promise<void> {
  await page.goto('/dashboard');
  await expect(page).toHaveURL(/\/login\?next=/);
  await expect(page.locator('#login_form')).toBeVisible();
  await page.getByLabel('Email').fill(email);
  await page.getByLabel('Password').fill(PASSWORD);
  await page.getByRole('button', { name: 'Log In' }).click();
  await expect(page).toHaveURL(/\/dashboard$/);
}

test.describe('quiz-taking journey (#109)', () => {
  test('student takes the quiz, submits, and sees the score + pending essay', async ({ page }) => {
    await login(page, STUDENT_EMAIL);

    // Quiz list: published quiz visible, no "+ Quiz" for the student.
    await page.goto(`/courses/${COURSE_ID}/quizzes`);
    await expect(page.locator('#quizzes_index')).toBeVisible();
    const row = page.locator(`#quiz_row_${QUIZ_ID}`);
    await expect(row).toBeVisible();
    await expect(page.locator('#new_quiz_link')).toHaveCount(0);

    // Show page -> Take Quiz.
    await row.locator('a.quiz-title-link').click();
    await expect(page).toHaveURL(new RegExp(`/courses/${COURSE_ID}/quizzes/${QUIZ_ID}/?$`));
    await expect(page.locator('#quiz_details')).toBeVisible();
    await page.locator('#take_quiz_link').click();

    // Take page: one form, DOM contract field names.
    await expect(page).toHaveURL(new RegExp(`/courses/${COURSE_ID}/quizzes/${QUIZ_ID}/take$`));
    await expect(page.locator('#submit_quiz_form')).toBeVisible();
    await expect(page.locator(`#question_${MC_ID}`)).toBeVisible();
    await expect(page.locator(`#question_${TF_ID}`)).toBeVisible();
    await expect(page.locator(`#question_${ESSAY_ID}`)).toBeVisible();

    // Answer: MC correct (radio value 1), true_false correct (11), essay text.
    await page
      .locator(`input[name="question_${MC_ID}"][value="1"]`)
      .check();
    await page
      .locator(`input[name="question_${TF_ID}"][value="11"]`)
      .check();
    await page
      .locator(`textarea[name="question_${ESSAY_ID}"]`)
      .fill('My essay answer');

    await page.locator('#submit_quiz_button').click();

    // Results: MC + TF auto-graded (20/30), essay pending review.
    await expect(page).toHaveURL(
      new RegExp(`/courses/${COURSE_ID}/quizzes/${QUIZ_ID}/submissions/\\d+$`),
    );
    await expect(page.locator('#score_summary')).toContainText(
      'Score for this attempt: 20 / 30',
    );
    await expect(page.locator('#pending_review_note')).toBeVisible();
    await expect(
      page.locator(`#question_${MC_ID}.correct, #question_${MC_ID} .outcome_correct`),
    ).toBeVisible();
    await expect(page.locator('#quiz_results')).toContainText('My essay answer');
  });

  test('teacher sees the submissions table with the attempt score', async ({ page }) => {
    await login(page, TEACHER_EMAIL);

    await page.goto(`/courses/${COURSE_ID}/quizzes/${QUIZ_ID}`);
    await expect(page.locator('#quiz_submissions_table')).toBeVisible();
    const row = page.locator('#quiz_submissions_table tbody tr').first();
    await expect(row).toContainText('Quiz Smoke Student');
    await expect(row).toContainText('20');

    // Teacher can open the student's results.
    await row.locator('a.submission-link').click();
    await expect(page).toHaveURL(
      new RegExp(`/courses/${COURSE_ID}/quizzes/${QUIZ_ID}/submissions/\\d+$`),
    );
    await expect(page.locator('#score_summary')).toContainText(
      'Score for this attempt: 20 / 30',
    );
  });
});
