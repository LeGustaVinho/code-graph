const { add } = require('./lib');

function total() {
  const lib = require('./lib');
  return add(1, 2) + lib.sub(3, 1);
}

module.exports = total;
