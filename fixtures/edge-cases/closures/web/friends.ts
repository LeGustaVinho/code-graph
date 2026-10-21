import { protectedRequest } from './api';

export function addFriend(code: string) {
  return protectedRequest('friend', '/v1/friends', { code });
}
