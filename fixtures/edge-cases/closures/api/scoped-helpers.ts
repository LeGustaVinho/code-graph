import Fastify from 'fastify';

// Two independent lexical scopes deliberately reuse the same helper names.
export function buildReal(env: NodeJS.ProcessEnv = process.env) {
  const app = Fastify();
  const route = (method: 'GET' | 'POST', url: string, handler: () => unknown) =>
    app.route({ method, url, handler });
  const required = (name: string) => env[name];
  route('GET', '/genuine', () => required('GENUINE_KEY'));
  return app;
}

export function buildUnrelated(cache: Map<string, unknown>) {
  const route = (_method: string, path: string, callback: () => unknown) =>
    cache.set(path, callback());
  const required = (name: string) => cache.get(name);
  route('POST', '/phantom', () => required('NOT_AN_ENV_KEY'));
  return cache;
}
