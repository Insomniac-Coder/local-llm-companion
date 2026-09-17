import test from 'node:test';
import assert from 'node:assert/strict';
import { loadErrorSummary } from '../src/services/loadError.ts';

// The shape of a real failure: the sidecar error, the app's explanation, then
// the runtime's log excerpt that used to fill the machine panel.
const failure = 'could not start inference: Generation failed: llama-server exited before becoming ready (exit code: 1). '
  + "The model file is missing metadata this runtime requires for its architecture. Files taken from Ollama's registry are packaged for Ollama. "
  + 'Runtime diagnostic: 0.00.173.451 I cmn common_params_print_info: verbosity = 3 0.00.703.479 E llama_model_load: error loading model: '
  + 'error loading model hyperparameters: key not found in model: gemma3.attention.layer_norm_rms_epsilon';

test('the machine panel shows the cause in one line, never the log excerpt', () => {
  const line = loadErrorSummary(failure);
  assert.equal(line, 'The model file is missing metadata this runtime requires for its architecture.');
  assert.ok(!line.includes('Runtime diagnostic'));
});

test('an unexplained error is shortened rather than dropped', () => {
  const raw = 'could not start inference: ' + 'x'.repeat(400) + ' Runtime diagnostic: log';
  const line = loadErrorSummary(raw);
  assert.ok(line.length <= 120);
  assert.ok(line.startsWith('could not start inference'));
  assert.ok(line.endsWith('…'));
  assert.equal(loadErrorSummary(''), 'The model could not be loaded.');
});
