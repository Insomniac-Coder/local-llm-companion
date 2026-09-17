// Inference benchmark harness: spawns llama-server with a flag set, runs a fixed
// prompt suite, and reports engine-measured timings (prompt/gen tok/s, cache hits).
import { spawn } from 'node:child_process';
import { readFileSync } from 'node:fs';

// Paths resolve against the repository root; override with BENCH_BIN / BENCH_MODEL.
const ROOT = new URL('../../', import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, '$1');
// The project's runtime (scripts/build-runtime.*), not a path on one machine.
const SERVER_EXE = process.platform === 'win32' ? 'llama-server.exe' : 'llama-server';
const BIN = process.env.BENCH_BIN || ROOT + 'runtime/bin/' + SERVER_EXE;
// No default model: which file to measure is the caller's choice, never a
// model name baked into the harness.
const MODEL = process.env.BENCH_MODEL;
if (!MODEL) {
  console.error('Set BENCH_MODEL to the GGUF file to benchmark.');
  process.exit(2);
}
const PORT = 3999;
const CTX = process.env.BENCH_CTX || '32768';
const SYSTEM = readFileSync(new URL('./system_prompt.txt', import.meta.url), 'utf8');
const EDIT_FILE = readFileSync(new URL('./edit_fixture.rs', import.meta.url), 'utf8');
const PREFILL_FILE = readFileSync(new URL('./prefill_fixture.rs', import.meta.url), 'utf8');

const VARIANTS = {
  A_baseline: ['--batch-size', '512'],
  B_default_batch: [],
  C_cache_reuse: ['--cache-reuse', '256'],
  D_ngram_mod: ['--cache-reuse', '256', '--spec-type', 'ngram-mod'],
  E_ngram_map_k: ['--cache-reuse', '256', '--spec-type', 'ngram-map-k'],
  F_ngram_simple: ['--cache-reuse', '256', '--spec-type', 'ngram-simple'],
  G_kv_q8: ['--cache-reuse', '256', '--cache-type-k', 'q8_0', '--cache-type-v', 'q8_0'],
  H_ngram_mod_kv_q8: ['--cache-reuse', '256', '--spec-type', 'ngram-mod', '--cache-type-k', 'q8_0', '--cache-type-v', 'q8_0'],
};

const PROMPTS = [
  { name: 'chat', max: 300, user: 'Explain what a KV cache is in a transformer decoder, in about 250 words. Plain prose, no lists.' },
  { name: 'prefill6k', max: 80, user: 'Here is a Rust source file:\n\n```rust\n' + PREFILL_FILE + '\n```\n\nSummarize what this file does in exactly three bullet points.' },
  { name: 'edit', max: 1400, user: 'Here is a Rust source file:\n\n```rust\n' + EDIT_FILE + '\n```\n\nReturn the COMPLETE file unchanged except: rename the function `parse_hunk_header` to `parse_hunk_range` everywhere it appears. Output only the full file inside one ```rust code block, nothing else.' },
  { name: 'tooljson', max: 120, user: 'To answer my earlier question you must first read the file README.md. Reply with exactly one action envelope in this format and nothing else:\n```tool\n{"name":"read_file","args":{"path":"README.md"}}\n```' },
];

function baseArgs(extra) {
  const args = ['--host', '127.0.0.1', '--port', String(PORT), '--parallel', '1', '-m', MODEL,
    '--ctx-size', CTX, '--n-gpu-layers', 'auto', '--flash-attn', 'auto'];
  if (!extra.includes('--cache-type-k')) args.push('--cache-type-k', 'f16', '--cache-type-v', 'f16');
  return args.concat(extra);
}

async function waitHealth(child, ms = 180000) {
  const start = Date.now();
  while (Date.now() - start < ms) {
    if (child.exitCode !== null) throw new Error('server exited early (' + child.exitCode + ')');
    try {
      const r = await fetch('http://127.0.0.1:' + PORT + '/health');
      if (r.ok) return Date.now() - start;
    } catch {}
    await new Promise((r) => setTimeout(r, 250));
  }
  throw new Error('health timeout');
}

async function complete(user, max) {
  const t0 = performance.now();
  const r = await fetch('http://127.0.0.1:' + PORT + '/v1/chat/completions', {
    method: 'POST', headers: { 'content-type': 'application/json' },
    body: JSON.stringify({
      messages: [{ role: 'system', content: SYSTEM }, { role: 'user', content: user }],
      max_tokens: max, temperature: 0, top_k: 1, stream: false,
      chat_template_kwargs: { enable_thinking: false },
    }),
  });
  const wall = performance.now() - t0;
  if (!r.ok) throw new Error('HTTP ' + r.status + ': ' + (await r.text()).slice(0, 300));
  const v = await r.json();
  const t = v.timings || {};
  const text = (v.choices && v.choices[0] && v.choices[0].message && v.choices[0].message.content) || '';
  return {
    wall_ms: Math.round(wall), prompt_n: t.prompt_n, prompt_ms: t.prompt_ms, prompt_tps: t.prompt_per_second,
    predicted_n: t.predicted_n, predicted_ms: t.predicted_ms, gen_tps: t.predicted_per_second,
    cache_n: t.cache_n, draft_n: t.draft_n, draft_accepted: t.draft_n_accepted,
    finish: v.choices && v.choices[0] && v.choices[0].finish_reason, text_head: text.replace(/\s+/g, ' ').slice(0, 70),
    usage: v.usage, timings_keys: Object.keys(t).join(','),
  };
}

function pad(v, n) { return String(v).padStart(n); }

async function runVariant(name, extra) {
  const args = baseArgs(extra);
  const child = spawn(BIN, args, { stdio: ['ignore', 'pipe', 'pipe'] });
  let log = '';
  child.stdout.on('data', (d) => { log += d; });
  child.stderr.on('data', (d) => { log += d; });
  const rows = [];
  try {
    const health_ms = await waitHealth(child);
    const offload = (log.match(/offloaded (\d+)\/(\d+) layers to GPU/) || [])[0] || 'offload: ?';
    const kv = (log.match(/KV self size\s*=\s*[^\n]+/) || log.match(/CUDA0 KV buffer size\s*=\s*[^\n]+/) || [])[0] || '';
    const cuda = (log.match(/CUDA0 model buffer size\s*=\s*[^\n]+/) || [])[0] || '';
    console.log('\n=== ' + name + ' | health ' + health_ms + ' ms | ' + offload + ' | ' + kv.trim() + ' | ' + cuda.trim());
    console.log('args: ' + args.slice(11).join(' '));
    for (const p of PROMPTS) {
      for (const pass of ['cold', 'warm']) {
        const m = await complete(p.user, p.max);
        rows.push({ variant: name, prompt: p.name, pass, ...m });
        const spec = m.draft_n != null ? ' draft ' + m.draft_accepted + '/' + m.draft_n : '';
        console.log(p.name.padEnd(10) + ' ' + pass.padEnd(5) + ' prompt ' + pad(m.prompt_n, 5) + ' tok @ ' + pad(Number(m.prompt_tps || 0).toFixed(0), 5) + ' tok/s (' + Math.round(m.prompt_ms || 0) + ' ms, cache ' + (m.cache_n ?? '?') + ') | gen ' + pad(m.predicted_n, 4) + ' tok @ ' + pad(Number(m.gen_tps || 0).toFixed(1), 5) + ' tok/s | wall ' + m.wall_ms + ' ms' + spec + ' | ' + m.finish + ' | ' + m.text_head);
      }
    }
    console.log('timings keys: ' + rows[rows.length - 1].timings_keys);
  } catch (e) {
    console.log('!! ' + name + ' failed: ' + e.message + '\n' + log.slice(-2000));
  } finally {
    child.kill();
    await new Promise((r) => setTimeout(r, 1500));
  }
  return rows;
}

const wanted = process.argv.slice(2);
const all = [];
for (const [name, extra] of Object.entries(VARIANTS)) {
  if (wanted.length && !wanted.includes(name)) continue;
  all.push(...await runVariant(name, extra));
}
console.log('\nJSON_RESULTS ' + JSON.stringify(all));
