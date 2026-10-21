import type { Page } from '@playwright/test';

export class UsersPom {
  constructor(private page: Page) {}

  async open(): Promise<void> {
    await this.page.goto('/users');
  }
}
