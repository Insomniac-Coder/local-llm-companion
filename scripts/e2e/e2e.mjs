// End-to-end evaluation of chat and coding-agent behaviour through the real
// backend API, with a disposable fixture project and a scratch database.
import { readFileSync, writeFileSync, copyFileSync } from 'node:fs';
import { execFileSync } from 'node:child_process';

const API = process.env.COMPANION_API || 'http://127.0.0.1:5173';
const MODEL = process.argv[2] || 'qwen3-8b';
const FIXTURE = new URL('./fixture/', import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, '$1');
const report = [];
const log = (line) => { console.log(line); report.push(line); };

async function j(path, init) {
  const r = await fetch(API + path, { headers: { 'content-type': 'application/json' }, ...init });
  const text = await r.text();
  let body; try { body = JSON.parse(text); } catch { body = text; }
  if (!r.ok) throw new Error(`${init?.method || 'GET'} ${path} -> ${r.status}: ${typeof body === 'string' ? body.slice(0, 300) : JSON.stringify(body).slice(0, 300)}`);
  return body;
}

async function waitHealth() {
  for (let i = 0; i < 240; i++) {
    try { const r = await fetch(API + '/api/health'); if (r.ok) return; } catch {}
    await new Promise((r) => setTimeout(r, 500));
  }
  throw new Error('backend did not come up');
}

function sse(body, onEvent) {
  return new Promise(async (resolve, reject) => {
    try {
      const r = await fetch(API + '/api/chat', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) });
      if (!r.ok) return reject(new Error('chat ' + r.status + ' ' + (await r.text()).slice(0, 300)));
      const reader = r.body.getReader();
      const dec = new TextDecoder();
      let buf = '';
      for (;;) {
        const { done, value } = await reader.read();
        if (done) break;
        buf += dec.decode(value, { stream: true });
        let idx;
        while ((idx = buf.indexOf('\n\n')) >= 0) {
          const frame = buf.slice(0, idx); buf = buf.slice(idx + 2);
          let event = 'message'; const parts = [];
          for (const line of frame.split('\n')) {
            if (line.startsWith('event:')) event = line.slice(6).trim();
            else if (line.startsWith('data:')) { let v = line.slice(5); if (v.startsWith(' ')) v = v.slice(1); parts.push(v); }
          }
          onEvent(event, parts.join('\n'));
        }
      }
      resolve();
    } catch (e) { reject(e); }
  });
}

async function chat(convId, message, opts = {}) {
  const t0 = performance.now();
  let text = '', reasoning = '', done = null, errors = [], activities = [], firstToken = null, phases = [];
  await sse({ conversation_id: convId, message, classified: opts.classified ?? false, ...(opts.reasoning !== undefined ? { reasoning: opts.reasoning } : {}) }, (event, data) => {
    if (event === 'token') { if (firstToken === null) firstToken = performance.now() - t0; text += data; }
    else if (event === 'reasoning') reasoning += data;
    else if (event === 'done') done = JSON.parse(data);
    else if (event === 'error') errors.push(data);
    else if (event === 'activity') activities.push(JSON.parse(data));
    else if (event === 'phase') phases.push(JSON.parse(data).phase);
  });
  return { text, reasoning, done, errors, activities, phases, wall_ms: Math.round(performance.now() - t0), first_token_ms: firstToken == null ? null : Math.round(firstToken) };
}

function fmtTiming(done) {
  const t = done?.timing || {};
  return `engine ${t.engine_output_tps ?? '?'} tok/s out, prefill ${t.engine_prompt_tps ?? '?'} tok/s, cached ${t.cached_tokens ?? '?'}, predicted ${t.predicted_tokens ?? '?'}, visible ${t.output_tokens ?? '?'} (${t.token_basis}), drafted ${t.draft_accepted ?? '-'}/${t.draft_tokens ?? '-'}, first text ${t.first_visible_ms ?? '?'} ms, thinking ${t.thinking_ms ?? '-'} ms, total ${t.total_ms} ms, truncated=${done?.truncated}, reasoning=${done?.reasoning}`;
}

async function agentRun(workspacePath, convId, task, reasoning) {
  const t0 = performance.now();
  const start = await j('/api/agent/run', { method: 'POST', body: JSON.stringify({ workspace: workspacePath, task, mode: 'agent', conversation_id: convId, search: false, classified: true, reasoning }) });
  if (!start.run_id) return { start, events: [], wall_ms: 0 };
  const r = await fetch(API + '/api/agent/runs/' + start.run_id + '/events');
  const reader = r.body.getReader();
  const dec = new TextDecoder();
  let buf = '';
  const events = []; let deltas = 0; let deltaChars = 0; let firstDelta = null; const deltaText = {};
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    buf += dec.decode(value, { stream: true });
    let idx;
    while ((idx = buf.indexOf('\n\n')) >= 0) {
      const frame = buf.slice(0, idx); buf = buf.slice(idx + 2);
      let data = '';
      for (const line of frame.split('\n')) if (line.startsWith('data:')) { let v = line.slice(5); if (v.startsWith(' ')) v = v.slice(1); data += (data ? '\n' : '') + v; }
      if (!data) continue;
      const ev = JSON.parse(data);
      if (ev.kind === 'thought_delta') { deltas++; deltaChars += ev.message.length; deltaText[ev.iteration] = (deltaText[ev.iteration] || '') + ev.message; if (firstDelta === null) firstDelta = Math.round(performance.now() - t0); continue; }
      events.push(ev);
      if (['COMPLETED', 'FAILED', 'CANCELLED'].includes(ev.state)) { reader.cancel().catch(() => {}); return { start, events, deltas, deltaChars, deltaText, firstDelta, wall_ms: Math.round(performance.now() - t0) }; }
    }
  }
  return { start, events, deltas, deltaChars, deltaText, firstDelta, wall_ms: Math.round(performance.now() - t0) };
}

await waitHealth();
log(`\n##### MODEL ${MODEL} #####`);
const t0 = performance.now();
const loaded = await j('/api/models/load', { method: 'POST', body: JSON.stringify({ id: MODEL }) });
log(`load: ${JSON.stringify(loaded)} in ${Math.round(performance.now() - t0)} ms`);
const policy = await j('/api/runtime/policy');
log(`active policy: batch=${policy.active?.batch_size} spec=${policy.active?.speculative} reuse=${policy.active?.cache_reuse} kv=${policy.active?.cache_type_k} ctx=${policy.active?.effective_context}`);
const models = await j('/api/models');
const meta = models.find((m) => m.id === MODEL);
log(`metadata: reasoning=${meta?.supports_reasoning} tools=${meta?.tool_calling} vision=${meta?.vision} ctx=${meta?.context_length}`);
await j('/api/permissions/mode', { method: 'PUT', body: JSON.stringify({ mode: 'auto' }) });

// ---- Chat ----
const conv = await j('/api/conversations', { method: 'POST', body: JSON.stringify({ title: 'e2e chat', model_id: MODEL, mode: 'chat' }) });
let r = await chat(conv.id, 'What is the capital of France? Answer in one sentence.', { reasoning: false });
log(`\n[chat-1 reasoning off] ${r.wall_ms} ms, first token ${r.first_token_ms} ms, reasoning chars ${r.reasoning.length}, phases ${r.phases.join('>')}`);
log(`  answer: ${r.text.replace(/\s+/g, ' ').slice(0, 160)}`);
log(`  ${fmtTiming(r.done)}`);

r = await chat(conv.id, 'Now the same for Japan, and mention its largest island.', { reasoning: false });
log(`\n[chat-2 follow-up, cache check] ${r.wall_ms} ms, first token ${r.first_token_ms} ms`);
log(`  answer: ${r.text.replace(/\s+/g, ' ').slice(0, 160)}`);
log(`  ${fmtTiming(r.done)}`);

r = await chat(conv.id, 'Write a Python function that parses an ISO-8601 date string (YYYY-MM-DD) into (year, month, day) without external libraries, validating the calendar (leap years included), with a short docstring and two example asserts.', { reasoning: true });
log(`\n[chat-3 reasoning on, code] ${r.wall_ms} ms, first token ${r.first_token_ms} ms, reasoning chars ${r.reasoning.length}, phases ${r.phases.join('>')}`);
log(`  reasoning head: ${r.reasoning.replace(/\s+/g, ' ').slice(0, 140)}`);
log(`  answer head: ${r.text.replace(/\s+/g, ' ').slice(0, 200)}`);
log(`  has code block: ${/```python|```\n?def /.test(r.text)}, mentions leap: ${/leap/i.test(r.text)}`);
log(`  ${fmtTiming(r.done)}`);
// Check the produced Python actually runs when python is available.
const py = r.text.match(/```(?:python)?\n([\s\S]*?)```/);
if (py) {
  const path = new URL('./generated.py', import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, '$1');
  writeFileSync(path, py[1]);
  try { execFileSync('python', [path], { stdio: 'pipe', timeout: 20000 }); log('  generated python executed without error (asserts held)'); }
  catch (e) { log(`  generated python FAILED: ${(e.stderr || e.message).toString().slice(-300)}`); }
}

r = await chat(conv.id, 'Write a complete, well-commented Python implementation of an LRU cache class (get/put with O(1) operations using a doubly linked list and a dict), followed by a small test section that exercises eviction order. Do not abbreviate.', { reasoning: false });
log(`\n[chat-4 long output, old cap was 2048] ${r.wall_ms} ms, generated ${r.done?.generated_tokens} tokens, truncated=${r.done?.truncated}`);
log(`  ${fmtTiming(r.done)}`);

// ---- Code / Ask ----
copyFileSync(FIXTURE + 'calculator.js', FIXTURE + 'calculator.js.orig');
const ws = await j('/api/workspaces', { method: 'POST', body: JSON.stringify({ name: 'fixture', path: FIXTURE }) });
const codeConv = await j('/api/conversations', { method: 'POST', body: JSON.stringify({ title: 'e2e code', model_id: MODEL, mode: 'code', workspace: ws.id }) });
let tc = performance.now();
let cls = await j('/api/chat/classify', { method: 'POST', body: JSON.stringify({ conversation_id: codeConv.id, message: 'What does calculator.js export, and what does the test file check? Also, what is the support ticket number in the README?' }) });
log(`\n[ask-classify] ${JSON.stringify(cls)} in ${Math.round(performance.now() - tc)} ms`);
r = await chat(codeConv.id, 'What does calculator.js export, and what does the test file check? Also, what is the support ticket number in the README?', { classified: cls.source === 'model' });
const tools = r.activities.filter((a) => a.kind === 'tool_result' || a.kind === 'tool_error').map((a) => `${a.kind}:${a.tool}(${a.args?.path ?? a.args?.query ?? ''})`);
log(`[ask-answer] ${r.wall_ms} ms, tool rounds ${r.done?.tool_rounds}, tools: ${tools.join(', ')}${r.errors.length ? `, ERRORS: ${r.errors.join(' | ')}` : ''}`);
log(`  answer: ${r.text.replace(/\s+/g, ' ').slice(0, 400)}`);
log(`  correct: exports ${/add/.test(r.text) && /subtract/.test(r.text) && /multiply/.test(r.text)}, ticket ${/LANTERN-583/.test(r.text)}`);
log(`  ${fmtTiming(r.done)}`);

// ---- Agent ----
tc = performance.now();
const task = 'The subtract function in calculator.js is wrong: it adds instead of subtracting. Fix it with a minimal edit, then run `node calculator.test.js` and report the real result.';
cls = await j('/api/chat/classify', { method: 'POST', body: JSON.stringify({ conversation_id: codeConv.id, message: task }) });
log(`\n[agent-classify] ${JSON.stringify(cls)} in ${Math.round(performance.now() - tc)} ms`);
const run = await agentRun(FIXTURE, codeConv.id, task, false);
const last = run.events[run.events.length - 1];
const toolEvents = run.events.filter((e) => e.kind === 'tool_result' || e.kind === 'tool_error').map((e) => `${e.kind}:${e.tool}(${(e.args?.path ?? e.args?.command ?? e.args?.query ?? '').toString().slice(0, 40)})`);
log(`[agent] ${run.wall_ms} ms, final state ${last?.state}, iterations ${last?.iteration}, streamed deltas ${run.deltas} (${run.deltaChars} chars, first at ${run.firstDelta} ms)`);
log(`  tools: ${toolEvents.join(' | ')}`);
log(`  final: ${(run.events.find((e) => e.kind === 'final' || e.kind === 'error')?.message ?? '').replace(/\s+/g, ' ').slice(0, 300)}`);
const fixed = readFileSync(FIXTURE + 'calculator.js', 'utf8');
let testPass = false;
try { const out = execFileSync('node', ['calculator.test.js'], { cwd: FIXTURE, stdio: 'pipe' }).toString(); testPass = out.includes('CALCULATOR_TESTS_PASSED'); } catch { testPass = false; }
log(`  file fixed: ${/return a - b/.test(fixed)}, independent test run passes: ${testPass}`);
const contexts = run.events.filter((e) => e.kind === 'context' && e.context_usage?.prompt_tokens);
log(`  context per iteration (prompt tokens reported): ${contexts.map((e) => `${e.iteration}:${e.context_usage.prompt_tokens}`).join(' ')}`);
writeFileSync(new URL(`./agent-deltas-${MODEL}.txt`, import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, '$1'), Object.entries(run.deltaText || {}).map(([k, v]) => `===== iteration ${k} =====\n${v}`).join('\n\n'));
log(`  events: ${run.events.filter((e) => e.kind !== 'context').map((e) => `${e.iteration}:${e.kind}${e.tool ? '(' + e.tool + ')' : ''}`).join(' ')}`);
// restore fixture
copyFileSync(FIXTURE + 'calculator.js.orig', FIXTURE + 'calculator.js');

// Report
writeFileSync(new URL(`./report-${MODEL}.txt`, import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, '$1'), report.join('\n'));
