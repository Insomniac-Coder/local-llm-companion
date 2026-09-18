import test from 'node:test';
import assert from 'node:assert/strict';
import { applyAgentContext, contextDisplay } from '../src/services/contextUsage.ts';

const saved = { limit: 32768, estimated_tokens: 106, breakdown: { conversation: 106, attachments: 0, tools: 0, memory: 0, system: 0, output_reserve: 3276 } };
const usage = { estimated_tokens: 2500, prompt_tokens: null, generated_tokens: null, context_limit: 32768, output_reserve: 1024, turns: 5, pruned_turns: 0, images: 0, phase: 'request' };

test('saved text is explicitly estimated and a small nonzero amount is not shown as zero', () => {
  assert.equal(contextDisplay(saved).label, 'Saved context');
  assert.equal(contextDisplay(saved).estimated, true);
  assert.equal(contextDisplay(saved).percentLabel, '<1%');
});

test('agent input supersedes the small saved-history figure without adding them', () => {
  const context = applyAgentContext(saved, { state: 'PLANNING', iteration: 1, context_usage: usage }, 'run');
  assert.equal(context.estimated_tokens, 106);
  assert.equal(contextDisplay(context).tokens, 2500);
  assert.equal(contextDisplay(context).label, 'Agent input');
  assert.equal(contextDisplay(context).estimated, true);
});

test('model-reported prompt count wins and subsequent requests replace rather than accumulate', () => {
  let context = applyAgentContext(saved, { state: 'PLANNING', iteration: 1, context_usage: { ...usage, phase: 'response', prompt_tokens: 2870, generated_tokens: 90 } }, 'run');
  assert.equal(contextDisplay(context).tokens, 2870);
  assert.equal(contextDisplay(context).estimated, false);
  context = applyAgentContext(context, { state: 'PLANNING', iteration: 2, context_usage: { ...usage, estimated_tokens: 2100, pruned_turns: 3 } }, 'run');
  assert.equal(contextDisplay(context).tokens, 2100);
  assert.equal(contextDisplay(context).estimated, true);
});

test('completed runs are clearly historical; other run events cannot change them', () => {
  let context = applyAgentContext(saved, { state: 'PLANNING', iteration: 1, context_usage: usage }, 'run');
  context = applyAgentContext(context, { state: 'COMPLETED' }, 'unrelated');
  assert.equal(context.agent_context.active, true);
  context = applyAgentContext(context, { state: 'COMPLETED' }, 'run');
  assert.equal(contextDisplay(context).label, 'Last agent input');
});

test('new runs without input accounting show pending rather than fabricated zero or saved usage', () => {
  const context = { ...saved, agent_context: { active: true, run_id: 'new-run', iteration: 0, usage: null } };
  assert.equal(contextDisplay(context).tokens, null);
  assert.equal(contextDisplay(context).pending, true);
});

test('historical input uses its recorded window instead of a newly configured model window', () => {
  const context = { ...saved, limit: 4096, agent_context: { active: true, run_id: 'run', iteration: 1, usage: { ...usage, prompt_tokens: 3000, phase: 'response' } } };
  assert.equal(contextDisplay(context).limit, 32768);
  assert.equal(contextDisplay(context).health, 'healthy');
  assert.equal(contextDisplay(context).reserve, 1024);
  context.agent_context.usage.context_limit = 4096;
  assert.equal(contextDisplay(context).health, 'critical');
});

test('a changed compaction threshold shows at once, and a running agent keeps the one it started with', () => {
  // The setting is lowered from 90 to 80 while the last run's record still says 90.
  const idle = { ...saved, compact_at_pct: 80, agent_context: { active: false, run_id: 'run', iteration: 9, usage: { ...usage, compact_at_pct: 90 } } };
  assert.equal(contextDisplay(idle).compactAt, 80);
  const running = { ...idle, agent_context: { ...idle.agent_context, active: true } };
  assert.equal(contextDisplay(running).compactAt, 90, 'a run keeps the threshold it read when it started');
  assert.equal(contextDisplay({ ...idle, auto_compact: false }).compactAt, 0, 'off is off whatever was recorded');
  assert.equal(contextDisplay({ ...saved, compact_at_pct: 85 }).compactAt, 85, 'with no run at all the setting stands');
});
