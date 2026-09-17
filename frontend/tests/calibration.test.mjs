import test from 'node:test';
import assert from 'node:assert/strict';
import { performanceMode, placementLabel, profileSummary, staleReason, summarizeMicroBatches } from '../src/services/calibration.ts';

test('saves from before modes keep their meaning', () => {
  assert.equal(performanceMode({ runtime_auto: false }), 'manual');
  assert.equal(performanceMode({ runtime_auto: true }), 'auto');
  assert.equal(performanceMode({ runtime_auto: true, runtime: { mode: 'balanced' } }), 'balanced');
  assert.equal(performanceMode({ runtime: { mode: 'nonsense' } }), 'auto');
  assert.equal(performanceMode(null), 'auto');
});

test('a profile is described by what was measured', () => {
  const summary = profileSummary({
    name: 'balanced', threads: 16, threads_batch: 24, poll: 50, priority: 0, gpu_layers: 0,
    generation_tps: 34.09, prompt_tps: 441.5, generation_share: 95, free_cores_generating: 8, description: '',
  });
  assert.equal(summary, '34.1 tok/s generating (95%) · 442 tok/s prompts · 16 generation / 24 prompt threads · 8 cores free');
});

test('a calibration from other conditions says what changed', () => {
  const environment = { device_fingerprint: 'd', runtime_build: 'b1', power_source: 'ac', gpu_driver: '', cpu_topology: '' };
  const status = {
    calibration: { environment },
    current_environment: { ...environment, runtime_build: 'b2', power_source: 'battery' },
    comparable: false,
  };
  const reason = staleReason(status);
  assert.match(reason, /the runtime was rebuilt/);
  assert.match(reason, /power changed from ac to battery/);
  assert.equal(staleReason({ ...status, comparable: true }), null);
  assert.equal(staleReason({ calibration: null, current_environment: null, comparable: null }), null);
});

test('micro-batch sizes are averaged over their passes and compared with 512', () => {
  const rate = (median) => ({ median, p10: median, p90: median, samples: 3 });
  const run = (micro_batch, pass, prompt, generation, blocks) => ({
    micro_batch, pass, gpu_layers: 49, expert_blocks_on_cpu: blocks, threads: 24, prompt_tokens: 4096,
    prompt: prompt === null ? null : rate(prompt), generation: generation === null ? null : rate(generation),
  });
  const rows = summarizeMicroBatches([
    run(512, 0, 1600, 68, 25), run(1024, 0, 2400, 66, 26), run(2048, 0, 3100, 60, 28),
    run(2048, 1, 3082, 59.6, 28), run(1024, 1, 2338, 66.2, 26), run(512, 1, 1634, 67, 25),
  ]);
  assert.deepEqual(rows.map((row) => row.micro_batch), [512, 1024, 2048]);
  assert.equal(rows[0].generation, 67.5);
  assert.equal(rows[0].generation_change, null, '512 is the baseline');
  assert.equal(rows[1].prompt, 2369);
  assert.equal(rows[1].generation_change, -2.1);
  assert.equal(rows[1].prompt_change, 46.5);
  assert.equal(rows[2].generation_change, -11.4);
  assert.equal(rows[2].expert_blocks_on_cpu, 28);
  assert.deepEqual(summarizeMicroBatches(undefined), []);
  const without512 = summarizeMicroBatches([run(1024, 0, 2400, null, 26)]);
  assert.equal(without512[0].generation, null);
  assert.equal(without512[0].prompt_change, null, 'no baseline, no change');
});

test('placements read as the calibration measured them', () => {
  const plan = (placement, extra = {}) => ({ placement, threads: [24], gpu_layers: 49, prompt_tokens: 256, generated_tokens: 64, repetitions: 4, ...extra });
  assert.equal(placementLabel(plan('gpu')), 'every layer on the GPU');
  assert.equal(placementLabel(plan('cpu')), 'CPU only');
  assert.equal(placementLabel(plan('hybrid', { tensor_overrides: ['blk\.24\.ffn_up_exps=CPU'] })), 'every layer on the GPU, some expert weights in RAM');
  assert.equal(placementLabel(plan('hybrid', { gpu_layers: 41 })), '41 layers on the GPU, the rest on the CPU');
});
