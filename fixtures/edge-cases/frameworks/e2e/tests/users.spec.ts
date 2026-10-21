import { test, expect } from './fixtures';

test.beforeEach(async ({ usersPage }) => {
  await usersPage.open();
});

test('lists users', async ({ page, usersPage }) => {
  await page.goto('/users');
  await usersPage.open();
  await page.getByTestId('refresh-users').click();
  await expect(page.locator('[data-testid="settings-form"]')).toBeHidden();
});
