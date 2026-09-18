import test from 'node:test';
import assert from 'node:assert/strict';
import { initialHealth, MISSES_BEFORE_DOWN, modelStoppedDetail, nextHealth, shouldRecheck } from '../src/services/runtimeHealth.ts';

test('one failed check does not call the backend down; two in a row do', () => {
  const answering = nextHealth(initialHealth, true);
  assert.deepEqual(answering, { up: true, misses: 0 });
  const blip = nextHealth(answering, false);
  assert.equal(blip.up, true, 'a single failure (a laptop waking up) changes nothing on screen');
  assert.equal(shouldRecheck(blip), true, 'it is checked again shortly');
  const down = nextHealth(blip, false);
  assert.equal(MISSES_BEFORE_DOWN, 2);
  assert.equal(down.up, false);
  assert.equal(shouldRecheck(down), false, 'once down, the regular poll takes over');
});

test('an answer after a failure clears it', () => {
  const blip = nextHealth(nextHealth(initialHealth, true), false);
  assert.deepEqual(nextHealth(blip, true), { up: true, misses: 0 });
  const back = nextHealth(nextHealth(nextHealth(initialHealth, false), false), true);
  assert.deepEqual(back, { up: true, misses: 0 });
});

test('a backend that never answered is unknown after one failure, down after two', () => {
  const first = nextHealth(initialHealth, false);
  assert.equal(first.up, null);
  assert.equal(nextHealth(first, false).up, false);
});

test('the stopped model server is described from the backend report', () => {
  assert.equal(
    modelStoppedDetail('exit code 0xC0000005: it crashed (memory access violation)'),
    'It ended by itself (exit code 0xC0000005: it crashed (memory access violation)).',
  );
  assert.equal(modelStoppedDetail('exit code 7.'), 'It ended by itself (exit code 7).');
  assert.equal(modelStoppedDetail('  '), 'It ended by itself.');
});
