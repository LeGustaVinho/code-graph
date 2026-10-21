import express from 'express';

const router = express.Router();
const get = (path: string, handler: express.RequestHandler) => router.get(path, handler);

function listOrders() {
  return [];
}

get('/orders', (_req, res) => res.json(listOrders()));
export default router;
