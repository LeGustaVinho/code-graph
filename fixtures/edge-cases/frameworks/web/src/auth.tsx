import { createContext } from 'react';

export class Session {
  logout(): void {}
  refresh(): void {}
}

export const AuthContext = createContext<Session>(new Session());
