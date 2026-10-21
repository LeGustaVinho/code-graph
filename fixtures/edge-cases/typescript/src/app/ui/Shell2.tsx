import { lazy } from 'react';

const Panel = lazy(() => import('./Panel'));

export function Shell2() {
  return <Panel />;
}
