import { hash } from "./hash";

let currentToken: string | null = null;

export class AuthStore {
  private user: string | null = null;

  login(user: string, password: string): string {
    this.user = user;
    currentToken = hash(user + ":" + password);
    return currentToken;
  }

  logout(): void {
    this.user = null;
    currentToken = null;
  }
}
