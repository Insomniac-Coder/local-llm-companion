import test from 'node:test';
import assert from 'node:assert/strict';
import { chartSegments, formatReading, isReading } from '../src/components/resourceTelemetry.ts';
import { highlightCode } from '../src/components/syntaxHighlighter.ts';

const sample = (ts, cpu_pct) => ({ ts, cpu_pct, ram_used_gb: null, ram_total_gb: null, gpu_pct: null, vram_used_gb: null, vram_total_gb: null, gpu_temp_c: null, gpu_power_w: null });

test('unavailable and invalid telemetry is distinct from genuine zero', () => {
  for (const value of [null, undefined, NaN, Infinity, -1, '0']) {
    assert.equal(isReading(value), false);
    assert.equal(formatReading(value), '—');
  }
  assert.equal(formatReading(0), '0');
  assert.equal(formatReading(1.25, 1), '1.3');
});

test('missing samples split the chart without fabricated zero readings', () => {
  const segments = chartSegments([sample(100, 0), sample(150, null), sample(200, 100)], 'cpu_pct', 100, 100, 200);
  assert.equal(segments.length, 2);
  assert.deepEqual(segments[0][0], { x: 0, y: 140, value: 0, ts: 100 });
  assert.deepEqual(segments[1][0], { x: 600, y: 4, value: 100, ts: 200 });
});

test('chart spacing follows timestamps rather than array indexes', () => {
  const points = chartSegments([sample(100, 10), sample(110, 20), sample(200, 30)], 'cpu_pct', 100, 100, 200)[0];
  assert.deepEqual(points.map((point) => point.x), [0, 60, 600]);
});

test('empty and single-sample charts remain finite with zero capacity', () => {
  assert.deepEqual(chartSegments([], 'cpu_pct', 0, 0, 0), []);
  const point = chartSegments([sample(100, 0)], 'cpu_pct', 0, 100, 100)[0][0];
  assert.ok(Number.isFinite(point.x) && Number.isFinite(point.y));
});

test('lazy highlighting supports common aliases and concurrent loads', async () => {
  const results = await Promise.all(['js', 'jsx', 'javascript'].map((language) => highlightCode('const answer = 42;', language)));
  assert.ok(results.every((html) => html.includes('hljs-keyword')));
  assert.equal(new Set(results).size, 1);
});

test('uncommon-language fallback preserves highlighting and unknown code is escaped', async () => {
  assert.match(await highlightCode('puts "hello"', 'ruby'), /hljs-/);
  assert.equal(await highlightCode('<script>&</script>', 'unknown-language'), '&lt;script&gt;&amp;&lt;/script&gt;');
  assert.equal(await highlightCode('<b>hello</b>', 'plaintext'), '&lt;b&gt;hello&lt;/b&gt;');
});
