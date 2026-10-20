// A lazy import loads './admin' on demand; it does not re-export admin's names.
export const loadAdmin = () => import('./admin');

export function secret(): void {}
