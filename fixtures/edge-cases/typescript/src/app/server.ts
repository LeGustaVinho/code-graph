import express from 'express';
import Fastify from 'fastify';

const router = express.Router();
const fastify = Fastify();

router.route('/items').get((req, res) => {
  res.send([]);
});

fastify.get('/health', async () => ({ ok: true }));
