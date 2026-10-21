// `@acme/util/fmt` is mapped by the package's `exports`, not by its folder layout.
import { fmtFn } from '@acme/util/fmt';

export function useFmt(): string {
  return fmtFn();
}
