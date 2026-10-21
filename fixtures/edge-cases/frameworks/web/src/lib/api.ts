// `/api` is proxied to the backend without the prefix (vite.config.ts).
export async function listUsers() {
  const res = await fetch('/api/users');
  return res.json();
}
