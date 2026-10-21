import Fastify from 'fastify';
import autoload from '@fastify/autoload';
import path from 'node:path';
import { Db } from './db';

export const app = Fastify();

app.decorate('db', new Db());
app.decorate('authenticate', async (request) => {
  audit();
});

app.addHook('onRequest', async (request, reply) => {
  audit();
});

app.register(autoload, { dir: path.join(__dirname, 'routes'), options: { prefix: '' } });

function audit(): void {}
