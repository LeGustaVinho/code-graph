import { test as base } from '@playwright/test';
import { UsersPom } from './pom';

type Fixtures = { usersPage: UsersPom };

export const test = base.extend<Fixtures>({
  usersPage: async ({ page }, use) => {
    await use(new UsersPom(page));
  },
});

export { expect } from '@playwright/test';
