const base = import.meta.env.VITE_API_URL ?? '';

export async function request<T>(method: string, path: string, body?: unknown): Promise<T> {
  const response = await fetch(`${base}${path}`, { method, body: JSON.stringify(body) });
  return (await response.json()) as T;
}

export async function protectedRequest<T>(action: string, path: string, body: Record<string, unknown>): Promise<T> {
  await request('POST', '/v1/auth/challenge', { action });
  return request('POST', path, body);
}

export const me = () => request('GET', '/v1/me');
