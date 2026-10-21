import { Router } from 'express';

const members = Router();

members.get('/:id', (req, res) => {
  res.send({});
});

export default members;
