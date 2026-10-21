import { UsersService, Entity, OrderDto, Status } from '@app/services/users';
import { shared } from '@shared';
import { utilFn } from '@acme/util';
import { math } from './barrel';
import { outer } from './aliasx';
import { secret } from './lazy';
import Late, { early, renamed } from './hoisted';

export function Roles(...roles: string[]) {
  return (target: unknown) => target;
}

export class Controller2 {
  constructor(private users: UsersService) {}

  @Roles('admin')
  load(): void {
    this.users.find('1');
    shared();
    utilFn();
    math.sq(2);
    outer();
    secret();
  }
}

export function save<T extends Entity>(x: T): void {
  x.persist();
}

export function show(o: OrderDto): void {
  const s = Status.Active;
  const providers = [UsersService];
}

function getSvc(): UsersService {
  return new UsersService();
}

export function useReturn(): void {
  const svc = getSvc();
  svc.find('x');
}

function helper(): number {
  return 0;
}

export function shadow(): number {
  const helper = () => 1;
  return helper();
}

export class Base0 {
  constructor(a: number) {}
}

export class Kid extends Base0 {
  constructor() {
    super(1);
  }
}

export function useHoisted(): void {
  early();
  renamed();
  new Late().run();
}
