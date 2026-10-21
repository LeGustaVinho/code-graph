class Ledger {
  post() {}
}

/** @param {Ledger} ledger */
function book(ledger) {
  ledger.post();
}

/** @type {Ledger} */
const main = makeLedger();

function flush() {
  main.post();
}

function Account() {}

Account.prototype.close = function () {
  this.audit();
};

Account.prototype.audit = function () {};

module.exports = { book, flush, Account };
