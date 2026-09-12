import test from 'node:test';
import assert from 'node:assert/strict';
import { VisibleOutputMeter } from './outputTiming.ts';

test('output meter starts at the first nonempty text, never at processing or an empty chunk', () => {
  const meter = new VisibleOutputMeter();
  assert.equal(meter.append('', 0), null);
  assert.equal(meter.append('abcd', 10_000), null);
  assert.equal(meter.append('efgh', 11_000), 2);
});

test('tool and thinking waits between output rounds do not dilute the live estimate', () => {
  const meter = new VisibleOutputMeter();
  meter.append('abcd', 1_000);
  assert.equal(meter.append('efgh', 2_000), 2);
  meter.pause();
  meter.append('ijkl', 100_000);
  assert.equal(meter.append('mnop', 101_000), 2);
});

test('single-chunk output has no invented infinite speed', () => {
  const meter = new VisibleOutputMeter();
  assert.equal(meter.append('A complete buffered response', 1_000), null);
});
