import { test } from 'vitest';
import { app } from '../src/app';

test('GET /users', async () => {
  await app.inject({ method: 'GET', url: '/users' });
});
