const assert = require('node:assert/strict');
const { add, subtract, multiply } = require('./calculator');
assert.equal(add(2, 3), 5);
assert.equal(subtract(10, 4), 6);
assert.equal(multiply(3, 4), 12);
console.log('CALCULATOR_TESTS_PASSED');
