import test from 'node:test';
import assert from 'node:assert/strict';
import { cacheDetails } from '../src/components/cacheDiagnostics.ts';

test('legacy automatic cache budgets never become a claimed allocation manager', () => {
  const details = cacheDetails({ configured_context: 8192, budgets: { ram: 'auto', vram: 'auto' } });
  assert.equal(details.capacity, '8,192 token capacity');
  assert.equal(details.runtimeSize, 'Not measured');
  assert.doesNotMatch(JSON.stringify(details), /Automatic|Runtime-managed budget/);
  assert.match(details.switchNote, /recreates.*temporary.*cache/);
  assert.match(details.switchNote, /Saved conversations, attachments and memory stay/);
});

test('missing, zero and measured cache readings remain distinct', () => {
  for (const usage_bytes of [undefined, null, NaN, -1, Infinity]) {
    assert.equal(cacheDetails({ kv_cache: { usage_bytes } }).runtimeSize, 'Not measured');
  }
  assert.equal(cacheDetails({ configured_context: 0 }).capacity, 'No context allocated');
  assert.equal(cacheDetails({ kv_cache: { usage_bytes: 0 } }).runtimeSize, '0.0 MB');
  assert.equal(cacheDetails({ kv_cache: { usage_bytes: 1_048_576 } }).runtimeSize, '1.0 MB');
});
