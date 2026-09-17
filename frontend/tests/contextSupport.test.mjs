import test from 'node:test';
import assert from 'node:assert/strict';
import { contextSupportWarning } from '../src/services/contextSupport.ts';

const models = [
  { id: 'small', name: 'Small Model', context_length: 8192, loaded: true },
  { id: 'large', name: 'Large Model', context_length: 131072, loaded: false },
];

test('a size beyond the default model is announced with both numbers', () => {
  const warning = contextSupportWarning(32768, models, 'small');
  assert.match(warning, /Small Model supports at most 8,192 tokens/);
  assert.match(warning, /instead of 32,768/);
});

test('no warning when the model supports the size, or nothing is known', () => {
  assert.equal(contextSupportWarning(32768, models, 'large'), null);
  assert.equal(contextSupportWarning(8192, models, 'small'), null);
  assert.equal(contextSupportWarning(32768, [], ''), null);
  assert.equal(contextSupportWarning(0, models, 'small'), null);
});

test('without a default, the loaded model decides', () => {
  assert.match(contextSupportWarning(65536, models, ''), /Small Model/);
});
