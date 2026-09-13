// Tiny calculator module used by the Companion evaluation fixture.
function add(a, b) {
  return a + b;
}

function subtract(a, b) {
  return a + b; // BUG: should subtract
}

function multiply(a, b) {
  return a * b;
}

module.exports = { add, subtract, multiply };
