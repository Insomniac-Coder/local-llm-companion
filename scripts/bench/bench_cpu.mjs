// CPU-only benchmark: forces the bundled runtime onto the CPU path (--device none)
// exactly as the app's automatic CPU fallback does, then compares launch flags.
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
const PORT = 3998;
const SYSTEM = readFileSync(new URL('./system_prompt.txt', import.meta.url), 'utf8');
const EDIT_FILE = readFileSync(new URL('./edit_fixture.rs', import.meta.url), 'utf8').split('\n').slice(0, 60).join('\n');
const PREFILL_FILE = readFileSync(new URL('./prefill_fixture.rs', import.meta.url), 'utf8').slice(0, 7000);
const THREADS = process.env.BENCH_THREADS ? process.env.BENCH_THREADS.split(',') : [];

const CPU_BASE = ['--device', 'none', '--no-op-offload', '--no-kv-offload', '--n-gpu-layers', '0', '--ctx-size', '8192'];
const VARIANTS = {
  CPU_A_app_current: ['--batch-size', '128', '--flash-attn', 'off', '--cache-type-k', 'f16', '--cache-type-v', 'f16'],
  CPU_B_default_batch_fa_off: ['--flash-attn', 'off', '--cache-type-k', 'f16', '--cache-type-v', 'f16'],
  CPU_C_default_batch_fa_auto: ['--flash-attn', 'auto', '--cache-type-k', 'f16', '--cache-type-v', 'f16'],
  CPU_D_ngram_simple: ['--flash-attn', 'auto', '--cache-type-k', 'f16', '--cache-type-v', 'f16', '--cache-reuse', '256', '--spec-type', 'ngram-simple'],
  CPU_E_kv_q8: ['--flash-attn', 'auto', '--cache-type-k', 'q8_0', '--cache-type-v', 'q8_0', '--cache-reuse', '256'],
};
for (const t of THREADS) {
  VARIANTS['CPU_T' + t] = ['--flash-attn', 'auto', '--cache-type-k', 'f16', '--cache-type-v', 'f16', '--threads', t];
}

const PROMPTS = [
  { name: 'chat', max: 120, user: 'Explain what a KV cache is in a transformer decoder, in about 100 words. Plain prose, no lists.' },
  { name: 'prefill2k', max: 40, user: 'Here is a Rust source file:\n\n```rust\n' + PREFILL_FILE + '\n```\n\nSummarize what this file does in one sentence.' },
  { name: 'edit', max: 700, user: 'Here is a Rust source file:\n\n```rust\n' + EDIT_FILE + '\n```\n\nReturn the COMPLETE file unchanged except: rename the function `parse_hunk_header` to `parse_hunk_range` everywhere it appears. Output only the full file inside one ```rust code block, nothing else.' },
  { name: 'tooljson', max: 60, user: 'To answer my earlier question you must first read the file README.md. Reply with exactly one action envelope in this format and nothing else:\n```tool\n{"name":"read_file","args":{"path":"README.md"}}\n```' },
];

async function waitHealth(child, ms = 300000) {
  const start = Date.now();
  while (Date.now() - start < ms) {
    if (child.exitCode !== null) throw new Error('server exited early (' + child.exitCode + ')');
    try { const r = await fetch('http://127.0.0.1:' + PORT + '/health'); if (r.ok) return Date.now() - start; } catch {}
    await new Promise((r) => setTimeout(r, 250));
  }
  throw new Error('health timeout');
}

async function complete(user, max) {
  const t0 = performance.now();
  const r = await fetch('http://127.0.0.1:' + PORT + '/v1/chat/completions', {
    method: 'POST', headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ messages: [{ role: 'system', content: SYSTEM }, { role: 'user', content: user }], max_tokens: max, temperature: 0, top_k: 1, stream: false, chat_template_kwargs: { enable_thinking: false } }),
  });
  const wall = performance.now() - t0;
  if (!r.ok) throw new Error('HTTP ' + r.status + ': ' + (await r.text()).slice(0, 300));
  const v = await r.json();
  const t = v.timings || {};
  const text = (v.choices && v.choices[0] && v.choices[0].message && v.choices[0].message.content) || '';
  return { wall_ms: Math.round(wall), prompt_n: t.prompt_n, prompt_ms: t.prompt_ms, prompt_tps: t.prompt_per_second, predicted_n: t.predicted_n, gen_tps: t.predicted_per_second, cache_n: t.cache_n, draft_n: t.draft_n, draft_accepted: t.draft_n_accepted, finish: v.choices && v.choices[0] && v.choices[0].finish_reason, text_head: text.replace(/\s+/g, ' ').slice(0, 60) };
}

function pad(v, n) { return String(v).padStart(n); }

async function runVariant(name, extra) {
  const args = ['--host', '127.0.0.1', '--port', String(PORT), '--parallel', '1', '-m', MODEL, ...CPU_BASE, ...extra];
  const child = spawn(BIN, args, { stdio: ['ignore', 'pipe', 'pipe'] });
  let log = '';
  child.stdout.on('data', (d) => { log += d; });
  child.stderr.on('data', (d) => { log += d; });
  const rows = [];
  try {
    const health_ms = await waitHealth(child);
    const threads = (log.match(/n_threads\s*=\s*\d+[^\n]*/) || [])[0] || '';
    console.log('\n=== ' + name + ' | health ' + health_ms + ' ms | ' + threads.trim());
    console.log('args: ' + extra.join(' '));
    for (const p of PROMPTS) {
      for (const pass of ['cold', 'warm']) {
        const m = await complete(p.user, p.max);
        rows.push({ variant: name, prompt: p.name, pass, ...m });
        const spec = m.draft_n != null ? ' draft ' + m.draft_accepted + '/' + m.draft_n : '';
        console.log(p.name.padEnd(10) + ' ' + pass.padEnd(5) + ' prompt ' + pad(m.prompt_n, 5) + ' tok @ ' + pad(Number(m.prompt_tps || 0).toFixed(1), 6) + ' tok/s (cache ' + (m.cache_n ?? '?') + ') | gen ' + pad(m.predicted_n, 4) + ' tok @ ' + pad(Number(m.gen_tps || 0).toFixed(2), 6) + ' tok/s | wall ' + m.wall_ms + ' ms' + spec + ' | ' + m.finish + ' | ' + m.text_head);
      }
    }
  } catch (e) {
    console.log('!! ' + name + ' failed: ' + e.message + '\n' + log.slice(-1500));
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
