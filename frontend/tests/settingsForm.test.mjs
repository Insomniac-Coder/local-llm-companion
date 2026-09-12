import test from 'node:test';
import assert from 'node:assert/strict';
import { updateSetting, settingsSearchMatches, expertSectionOpen } from '../src/components/settingsForm.ts';

test('simplified settings preserve inactive preferences and unrelated manual overrides', () => {
  const source = { advanced: { kv_cache_type: 'Q8_0' }, privacy: { telemetry: false }, hardware: { cpu_threads: 6 }, runtime_auto: false };
  const changed = updateSetting(source, ['runtime_auto'], true);
  assert.equal(source.runtime_auto, false);
  assert.equal(changed.runtime_auto, true);
  assert.deepEqual(changed.advanced, source.advanced);
  assert.deepEqual(changed.privacy, source.privacy);
  assert.deepEqual(changed.hardware, source.hardware);
  assert.equal(updateSetting(changed, ['appearance', 'theme'], 'dark').appearance.theme, 'dark');
});

test('expert search opens matching collapsed controls without changing manual open state', () => {
  assert.equal(expertSectionOpen(false, 'Context size Temperature', ' temperature '), true);
  assert.equal(expertSectionOpen(false, 'Context size Temperature', ''), false);
  assert.equal(expertSectionOpen(false, 'Context size Temperature', 'theme'), false);
  assert.equal(expertSectionOpen(true, 'Context size Temperature', ''), true);
  assert.equal(settingsSearchMatches('Manage hardware automatically', 'HARDWARE'), true);
  assert.equal(settingsSearchMatches('Manage hardware automatically', 'missing'), false);
});
