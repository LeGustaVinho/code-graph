import express from 'express';
import members from './routes/members';

const app = express();
const v2 = express.Router();

v2.get('/status', (req, res) => {
  res.send('ok');
});

app.use('/api/v2', v2);
app.use('/api/members', members);
