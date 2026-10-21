export interface Repo {
  transaction<T>(work: (tx: Repo) => Promise<T>): Promise<T>;
  one(sql: string): Promise<unknown>;
}

export class PgRepo implements Repo {
  async transaction<T>(work: (tx: Repo) => Promise<T>): Promise<T> {
    return work(this);
  }
  async one(sql: string): Promise<unknown> {
    return sql;
  }
}

export class BaseService {
  constructor(readonly repo: Repo) {}
  async rate(tx: Repo, bucket: string, limit: number) {
    await tx.one(`${bucket}:${limit}`);
  }
}
class L1 extends BaseService {}
class L2 extends L1 {}
class L3 extends L2 {}
class L4 extends L3 {}
class L5 extends L4 {}
class L6 extends L5 {}
class L7 extends L6 {}
export class AppService extends L7 {
  async me(id: string) {
    await this.rate(this.repo, 'me', 2);
    await this.rate(this.repo, 'me-burst', 10);
    return id;
  }
  async friend(id: string) {
    if (!id) throw new Error('NO_ID');
    return this.repo.transaction(async (tx) => tx.one(id));
  }
}
