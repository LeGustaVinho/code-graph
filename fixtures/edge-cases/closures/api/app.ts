import Fastify from 'fastify';
import { AppService } from './service.js';

export function buildApp(service: AppService) {
  const app = Fastify();
  app.setErrorHandler((error, _request, reply) => reply.code(500).send({ error: String(error) }));
  async function actor(request: { headers: Record<string, string> }) {
    return request.headers.authorization;
  }
  const route = (method: 'GET' | 'POST', url: string, handler: (id: string) => Promise<unknown>) =>
    app.route({
      method,
      url,
      handler: async (request) => {
        const id = await actor(request as never);
        await service.repo.transaction((tx) => service.rate(tx, `ip:${url}`, 100));
        return handler(String(id));
      },
    });
  route('GET', '/v1/me', (id) => service.me(id));
  route('POST', '/v1/friends', (id) => service.friend(id));
  app.get('/health', async () => ({ ok: true }));
  return app;
}
