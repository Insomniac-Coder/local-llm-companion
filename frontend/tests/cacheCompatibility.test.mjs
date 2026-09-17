import test from 'node:test';
import assert from 'node:assert/strict';
import { cacheConflict, flashAttentionRequired, quantizedCacheUnavailable } from '../src/services/cacheCompatibility.ts';
import { performanceMode } from '../src/services/calibration.ts';

const manual = (flash_attention, kv_cache) => ({ runtime_auto: false, runtime: { mode: 'manual', kv_cache }, hardware: { flash_attention } });
const mode = (settings) => performanceMode(settings);

test('manual mode without flash attention cannot pick the 8-bit cache', () => {
  assert.equal(quantizedCacheUnavailable(manual(false, 'f16'), 'manual'), true);
  assert.equal(quantizedCacheUnavailable(manual(true, 'f16'), 'manual'), false);
});

test('flash attention cannot be switched off while the 8-bit cache is selected', () => {
  assert.equal(flashAttentionRequired(manual(true, 'q8_0'), 'manual'), true);
  assert.equal(flashAttentionRequired(manual(true, 'f16'), 'manual'), false);
});

test('a conflict reached through a mode switch is reported and can be undone from either control', () => {
  const conflicted = manual(false, 'q8_0');
  assert.match(cacheConflict(conflicted, mode(conflicted)), /needs Flash Attention/);
  // Turning flash attention on is allowed; so is choosing f16.
  assert.equal(flashAttentionRequired(conflicted, 'manual'), false);
  assert.equal(cacheConflict(manual(true, 'q8_0'), 'manual'), null);
  assert.equal(cacheConflict(manual(false, 'f16'), 'manual'), null);
});

test('a save from before modes with the switch off counts as manual', () => {
  const legacy = { runtime_auto: false, runtime: { kv_cache: 'q8_0' }, hardware: { flash_attention: false } };
  assert.match(cacheConflict(legacy, mode(legacy)), /needs Flash Attention/);
});

test('modes that let the runtime choose flash attention have no conflict', () => {
  for (const name of ['auto', 'fastest', 'balanced', 'light']) {
    const settings = { runtime_auto: true, runtime: { mode: name, kv_cache: 'q8_0' }, hardware: { flash_attention: false } };
    assert.equal(cacheConflict(settings, mode(settings)), null, name);
    assert.equal(quantizedCacheUnavailable(settings, mode(settings)), false, name);
    assert.equal(flashAttentionRequired(settings, mode(settings)), false, name);
  }
});
