import { test, expect, type Page } from '@playwright/test';

// Journey 4 (issue #72): discussions + inbox.
//
// This spec is deliberately self-contained: the sentinel ids and credentials
// come from browser-smoke/fixtures/seed-discussions.sql (NOT helpers.ts /
// seed.sql), because that fragment owns the discussions/inbox fixture. The
// login flow is the same real form flow helpers.ts drives.

const DISCUSSIONS_TEACHER_EMAIL = 'discussions-teacher@test.local';
const DISCUSSIONS_STUDENT_EMAIL = 'discussions-student@test.local';
const SMOKE_PASSWORD = 'SmokeTest-123!';

const COURSE_ID = 993101; // Smoke Discussions Course
const COURSE_NAME = 'Smoke Discussions Course';
const TOPIC_ID = 993301; // "Welcome to the course discussion"
const TOPIC_TITLE = 'Welcome to the course discussion';
const TOPIC_BODY = 'Say hello and introduce yourself.';
const TEACHER_ENTRY = 'Teacher: welcome aboard, reply here!';
const STUDENT_ENTRY = 'Student: happy to be here.';
const CONVERSATION_ID = 993501; // "Smoke Inbox Subject"
const CONVERSATION_SUBJECT = 'Smoke Inbox Subject';
const CONVERSATION_MESSAGE = 'Student to teacher: do we need the textbook?';

async function login(page: Page, email: string): Promise<void> {
  await page.goto('/dashboard');
  await expect(page).toHaveURL(/\/login\?next=/);
  await expect(page.locator('#login_form')).toBeVisible();

  await page.getByLabel('Email').fill(email);
  await page.getByLabel('Password').fill(SMOKE_PASSWORD);
  await page.getByRole('button', { name: 'Log In' }).click();

  await expect(page).toHaveURL(/\/dashboard$/);
}

// The teacher journey creates the topic the student journey then reads, and
// the student's compose is verified from the teacher's second context, so the
// tests must run in order (fullyParallel would race them).
test.describe.configure({ mode: 'serial' });

test.describe('journey 4: teacher discussions + inbox', () => {
  test('teacher opens the seeded topic and replies; the reply renders', async ({ page }) => {
    await login(page, DISCUSSIONS_TEACHER_EMAIL);

    // Discussions index via the course: topic table renders the seeded topic.
    await page.goto(`/courses/${COURSE_ID}/discussions`);
    await expect(page.locator('#discussion_topics_table')).toBeVisible();
    const row = page.locator(`#discussion_topic_${TOPIC_ID}`);
    await expect(row).toBeVisible();
    await expect(
      row.locator('.discussion-topic-title'),
    ).toHaveText(TOPIC_TITLE);
    // The teacher authored the topic, and entries exist: reply count 2.
    await expect(row.locator('.topic-replies')).toHaveText('2');

    // Open the topic: body + both entries render.
    await row.locator('.discussion-topic-title').click();
    await expect(page).toHaveURL(
      new RegExp(`/courses/${COURSE_ID}/discussions/${TOPIC_ID}/?$`),
    );
    await expect(
      page.locator('.discussion-topic .message.user_content'),
    ).toContainText(TOPIC_BODY);
    await expect(page.getByText(TEACHER_ENTRY)).toBeVisible();
    await expect(page.getByText(STUDENT_ENTRY)).toBeVisible();

    // Reply to the student's entry through the inline form (parent_id).
    const studentEntry = page.locator('#entry_993402');
    await studentEntry
      .locator('textarea[name="message"]')
      .fill('Teacher reply via the browser smoke journey');
    await studentEntry.locator('button[type="submit"]').click();

    // PRG lands back on the topic; the reply is visible.
    await expect(page).toHaveURL(
      new RegExp(`/courses/${COURSE_ID}/discussions/${TOPIC_ID}/?$`),
    );
    await expect(
      page.getByText('Teacher reply via the browser smoke journey'),
    ).toBeVisible();
  });

  test('teacher creates a new topic through the form', async ({ page }) => {
    await login(page, DISCUSSIONS_TEACHER_EMAIL);

    await page.goto(`/courses/${COURSE_ID}/discussions`);
    await page.locator('#add_discussion').click();
    await expect(page).toHaveURL(
      new RegExp(`/courses/${COURSE_ID}/discussions/new$`),
    );

    await page.getByLabel('Title', { exact: true }).fill('Smoke Journey New Topic');
    await page
      .locator('#discussion_message')
      .fill('<p>Created by the browser-smoke teacher journey.</p>');
    await page.getByRole('button', { name: 'Create Discussion' }).click();

    // PRG lands on the new topic page.
    await expect(page).toHaveURL(
      new RegExp(`/courses/${COURSE_ID}/discussions/\\d+/?$`),
    );
    await expect(page.locator('.discussion-topic__title')).toHaveText(
      'Smoke Journey New Topic',
    );
    await expect(
      page.locator('.discussion-topic .message.user_content'),
    ).toContainText('Created by the browser-smoke teacher journey.');

    // And the index lists it (.first(): re-runs keep topics from earlier runs).
    await page.goto(`/courses/${COURSE_ID}/discussions`);
    await expect(
      page
        .locator('.discussion-topic-title', {
          hasText: 'Smoke Journey New Topic',
        })
        .first(),
    ).toBeVisible();
  });

  test('teacher inbox: unread conversation opens, gets a reply', async ({ page }) => {
    await login(page, DISCUSSIONS_TEACHER_EMAIL);

    // The global nav Inbox link targets /conversations and carries the
    // unread badge (the seeded conversation is unread for the teacher).
    const inboxLink = page.locator('#global_nav_conversations_link');
    await expect(inboxLink).toHaveAttribute('href', '/conversations');
    await expect(
      page.locator('#global_nav_conversations_unread_badge'),
    ).toBeVisible();

    await inboxLink.click();
    await expect(page).toHaveURL(/\/conversations\/?$/);

    // The seeded conversation renders unread-styled with the message preview.
    const row = page.locator(`#conversation_${CONVERSATION_ID}`);
    await expect(row).toBeVisible();
    await expect(row).toHaveClass(/(^|\s)unread(\s|$)/);
    await expect(row.locator('.conversation-item__subject')).toHaveText(
      CONVERSATION_SUBJECT,
    );
    await expect(row.locator('.conversation-item__preview')).toContainText(
      'do we need the textbook?',
    );

    // Open the thread: the message renders; the badge clears.
    await row.locator('.conversation-item__link').click();
    await expect(page).toHaveURL(
      new RegExp(`/conversations/${CONVERSATION_ID}/?$`),
    );
    await expect(page.getByText(CONVERSATION_MESSAGE)).toBeVisible();
    await expect(
      page.locator('.conversation-show__participants'),
    ).toContainText('Discussions Student');

    await page.goto('/conversations');
    await expect(
      page.locator('#global_nav_conversations_unread_badge'),
    ).toHaveCount(0);

    // Reply through the form; the message appears on the thread.
    await page.goto(`/conversations/${CONVERSATION_ID}`);
    await page
      .locator('#conversation_body')
      .fill('Teacher reply: yes, chapters one and two.');
    await page.getByRole('button', { name: 'Send' }).click();

    await expect(page).toHaveURL(
      new RegExp(`/conversations/${CONVERSATION_ID}/?$`),
    );
    await expect(
      page.getByText('Teacher reply: yes, chapters one and two.'),
    ).toBeVisible();
  });
});

test.describe('journey 4: student discussions + inbox', () => {
  test('student sees the new topic unread, opens it, composes to the teacher', async ({
    browser,
  }) => {
    // The student journey and the teacher verification run in ONE test with
    // two contexts, so the ordering is deterministic under fullyParallel.
    const student = await browser.newContext();
    const page = await student.newPage();
    await login(page, DISCUSSIONS_STUDENT_EMAIL);

    // Discussions index: no create button for a student.
    await page.goto(`/courses/${COURSE_ID}/discussions`);
    await expect(page.locator('#add_discussion')).toHaveCount(0);

    // A topic without a read row renders the unread marker.
    let unreadRow = page.locator('.discussion-topic-row.unread', {
      hasText: 'Smoke Journey New Topic',
    });
    await expect(unreadRow).toHaveCount(1);
    await expect(unreadRow.locator('.unread-mark')).toBeVisible();

    // Opening the topic clears the marker on the next index fetch.
    await unreadRow.locator('.discussion-topic-title').click();
    await expect(page).toHaveURL(
      new RegExp(`/courses/${COURSE_ID}/discussions/\\d+/?$`),
    );
    await page.goto(`/courses/${COURSE_ID}/discussions`);
    unreadRow = page.locator('.discussion-topic-row.unread', {
      hasText: 'Smoke Journey New Topic',
    });
    await expect(unreadRow).toHaveCount(0);

    // Inbox: compose a new conversation to the teacher.
    await page.goto('/conversations');
    await page.locator('#compose-btn').click();
    await expect(page).toHaveURL(/\/conversations\/compose$/);

    await page.getByLabel('Recipients').selectOption({
      label: 'Discussions Teacher <discussions-teacher@test.local>',
    });
    await page.getByLabel('Subject', { exact: true }).fill('Smoke compose subject');
    await page
      .locator('#compose_body')
      .fill('Student composed this in the browser smoke journey.');
    await page.getByRole('button', { name: 'Send' }).click();

    // PRG lands on the new thread with the composed message.
    await expect(page).toHaveURL(/\/conversations\/\d+\/?$/);
    await expect(
      page.getByText('Student composed this in the browser smoke journey.'),
    ).toBeVisible();

    // The unread filter scope still works for the student.
    await page.goto('/conversations?filter=unread');
    await expect(
      page.locator('.conversations__filter', { hasText: 'Unread' }),
    ).toHaveClass(/(^|\s)active(\s|$)/);

    // ---- Second context: the teacher sees the composed conversation ----
    const teacher = await browser.newContext();
    const teacherPage = await teacher.newPage();
    await login(teacherPage, DISCUSSIONS_TEACHER_EMAIL);

    await teacherPage.goto('/conversations');
    // The newest "Smoke compose subject" row is the one just composed; it
    // must be unread for the teacher. (.first(): re-runs keep read copies of
    // the same subject from earlier runs, ordered after the newest.)
    const composed = teacherPage
      .locator('.conversation-item.unread', {
        hasText: 'Smoke compose subject',
      })
      .first();
    await expect(composed).toBeVisible();

    // Mark all read clears the badge.
    await teacherPage.locator('#mark_all_read_btn').click();
    await expect(teacherPage).toHaveURL(/\/conversations\??$/);
    await expect(
      teacherPage.locator('#global_nav_conversations_unread_badge'),
    ).toHaveCount(0);

    await teacher.close();
    await student.close();
  });
});
