import test from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';
import { readFileSync } from 'node:fs';
import { build } from 'esbuild';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';

const sourcePath = fileURLToPath(new URL('../src/components/ModelLibraryItem.tsx', import.meta.url));
const result = await build({ entryPoints: [sourcePath], bundle: true, write: false, platform: 'node', format: 'cjs', jsx: 'automatic', external: ['react', 'react/*'], loader: { '.css': 'empty' } });
const loaded = { exports: {} };
new Function('require', 'module', 'exports', result.outputFiles[0].text)(createRequire(import.meta.url), loaded, loaded.exports);
const { default: ModelLibraryItem, ModelDetails } = loaded.exports;
const model = { id: 'alpha', name: 'Alpha model', parameters: '4B', quantization: 'Q4_K_M', context_length: 32768, vision: false, tool_calling: false, loaded: false };

test('each model owns an initially closed disclosure inside its own article', () => {
  const html = renderToStaticMarkup(createElement('div', null, [model, { ...model, id: 'beta', name: 'Beta model' }].map((item) => createElement(ModelLibraryItem, { key: item.id, model: item, loadingModel: false, onLoad() {}, onDelete() {}, notify() {} }))));
  const articles = [...html.matchAll(/<article\b[\s\S]*?<\/article>/g)].map((match) => match[0]);
  assert.equal(articles.length, 2);
  const ids = [];
  for (const article of articles) {
    assert.match(article, /aria-expanded="false"/);
    const id = /aria-controls="([^"]+)"/.exec(article)?.[1];
    assert.ok(id && article.includes(`id="${id}"`));
    assert.match(article, /hidden="" role="region"/);
    ids.push(id);
  }
  assert.notEqual(ids[0], ids[1]);
  assert.doesNotMatch(articles[0], /Beta model/);
});

test('detail content preserves file checks, capabilities, warnings and both recommendation tools', () => {
  const detail = { metadata: { ...model, architecture: 'llama' }, estimates: { file_gb: 2.4, gguf_present: true, projector_present: false, kv_cache_gb: 1.5, total_need_gb: 3.9, compat_warnings: ['Check available memory.'] }, recommended: { note: 'Use a smaller window.' } };
  const html = renderToStaticMarkup(createElement(ModelDetails, { detail, notify() {} }));
  for (const text of ['2.4 GB', 'GGUF', 'Not required · text model', 'Tools unverified', 'Reasoning unverified', 'Check available memory.', 'Use a smaller window.', 'Recommended context', 'Inference suggestions', 'not applied automatically', 'not measured usage']) assert.ok(html.includes(text), text);
  assert.doesNotMatch(html, /Validated preference|inference uses it/);
});

test('requests are invalidated on close/unmount and responses are checked against model identity', () => {
  const source = readFileSync(sourcePath, 'utf8');
  assert.match(source, /result\.metadata\.id !== model\.id/);
  assert.match(source, /return \(\) => \{ request\.current\+\+; \}/);
  assert.match(source, /const close = \(\) => \{\s*request\.current\+\+;/);
  assert.match(source, /ModelLibraryCard key=\{props\.model\.id\}/);
});
