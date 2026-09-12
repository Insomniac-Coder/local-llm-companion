import test from 'node:test';
import assert from 'node:assert/strict';
import { defaultModelOptions } from '../src/components/settingsModels.ts';

test('default model options show detected names while retaining IDs as values', () => {
  const models = [{ id: 'z-id', name: 'Zeta 8B' }, { id: 'a-id', name: 'Alpha 4B' }];
  assert.deepEqual(defaultModelOptions(models, 'z-id', 'ready'), [
    { value: '', label: 'Automatic selection' },
    { value: 'a-id', label: 'Alpha 4B' },
    { value: 'z-id', label: 'Zeta 8B' },
  ]);
  assert.equal(models[0].id, 'z-id', 'discovery order is not mutated');
});

test('missing saved model is preserved without replacing or exposing its internal ID as a name', () => {
  const options = defaultModelOptions([{ id: 'new', name: 'New model' }], 'old-id', 'ready');
  assert.deepEqual(options[1], { value: 'old-id', label: 'Previously selected model unavailable' });
  assert.equal(options[2].value, 'new');
});

test('loading and failed discovery do not falsely declare a saved model missing', () => {
  for (const state of ['loading', 'error']) {
    const options = defaultModelOptions([], 'saved-id', state);
    assert.equal(options[1].value, 'saved-id');
    assert.doesNotMatch(options[1].label, /Previously selected model unavailable/);
  }
  assert.deepEqual(defaultModelOptions([], '', 'ready'), [{ value: '', label: 'Automatic selection' }]);
});

test('model options do not duplicate a detected saved model', () => {
  const options = defaultModelOptions([{ id: 'a', name: 'Alpha' }, { id: 'a', name: 'Alpha' }], 'a', 'ready');
  assert.equal(options.filter((option) => option.value === 'a').length, 1);
});
