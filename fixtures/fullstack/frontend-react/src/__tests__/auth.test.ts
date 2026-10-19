import { AuthStore } from "../utils/auth";

describe("AuthStore", () => {
  it("returns a token on login", () => {
    const store = new AuthStore();
    const token = store.login("ana", "secret");
    expect(token).toBeTruthy();
  });
});
