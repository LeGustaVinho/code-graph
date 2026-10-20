namespace Util.Inner {
  export function f(): number {
    return 1;
  }
}

export function callNamespace(): number {
  return Util.Inner.f();
}

export class Overloaded {
  m(a: string): void;
  m(a: number): void;
  m(a: any): void {}
}

export function useOverloaded(o: Overloaded): void {
  o.m('x');
}

export class Counter {
  #count = 0;
  inc(): void {
    this.#count++;
  }
}

export class Cart {
  private items: number[] = [];
  get total(): number {
    return this.items.length;
  }
  set total(v: number) {}
}

export function readTotal(cart: Cart): number {
  return cart.total;
}

export const api = {
  list(id: string) {
    return id;
  },
  get: () => 1,
};

export function useApi(): string {
  return api.list('a');
}

export const Klass = class {
  run(): void {}
};

export class Columns {
  @Column() id: number;
}

declare function Column(): (t: unknown, k: string) => void;
