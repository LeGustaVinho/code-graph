export const HASH_ROUNDS = 10;

export function hash(value: string, rounds: number = HASH_ROUNDS): string {
  let h = value;
  for (let i = 0; i < rounds; i++) {
    h = btoa(h);
  }
  return h;
}

function unusedHelper(): number {
  return 42;
}
