import type { FastifyInstance } from 'fastify';

// Served at /users by @fastify/autoload (folder name).
export default async function (fastify: FastifyInstance) {
  fastify.get('/', async (request, reply) => {
    await fastify.authenticate(request);
    return fastify.db.findUsers();
  });
}
