export interface User {
  name: string;
}

export interface OrderDto {
  id: string;
}

export class Entity {
  persist(): void {}
}

export class UsersService {
  find(id: string): User {
    return { name: id };
  }
}

export enum Status {
  Active,
  Closed,
}
