export class Member {
  static find(id: string): Member {
    return new Member();
  }
  enroll(): void {}
  ship(): void {}
}

export class Roster {
  private members: Member[] = [];

  each(): void {
    this.members.forEach((m) => m.ship());
    for (const m of this.members) {
      m.enroll();
    }
  }
}

// `members.find` is Array.prototype.find, not the static `Member.find`.
export function pickMembers(members: Member[]): void {
  members.find((m) => m.enroll());
  members.map((m) => m.ship());
}
