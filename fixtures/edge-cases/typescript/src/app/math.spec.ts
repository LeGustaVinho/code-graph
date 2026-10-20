import { UsersService } from './services/users';

describe(UsersService.name, () => {
  it.each([1, 2])('adds %i', (n) => {
    expect(n).toBeGreaterThan(0);
  });
});
