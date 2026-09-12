import test from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';
import { build } from 'esbuild';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';

// Compile the actual TSX component in memory; no generated test files remain.
const result = await build({
  entryPoints: [fileURLToPath(new URL('../src/components/MessageView.tsx', import.meta.url))],
  bundle: true, write: false, platform: 'node', format: 'cjs', jsx: 'automatic',
  external: ['react', 'react/*', 'react-dom', 'react-dom/*', 'highlight.js', 'highlight.js/*'],
});
const loaded = { exports: {} };
new Function('require', 'module', 'exports', result.outputFiles[0].text)(createRequire(import.meta.url), loaded, loaded.exports);
const MessageView = loaded.exports.default;

test('ordinary code-chat answers render as conversation even with a saved response event', () => {
  const html = renderToStaticMarkup(createElement(MessageView, {
    role: 'assistant', text: 'RageV is a rendering project.',
    activities: [{ kind: 'response', state: 'COMPLETED', iteration: 1, message: 'RageV is a rendering project.' }],
  }));
  assert.match(html, /<p>RageV is a rendering project\.<\/p>/);
  assert.doesNotMatch(html, /structured-message|Step 1|Open agent activity/);
});

test('language fences have one rich block, not nested pre elements', () => {
  const html = renderToStaticMarkup(createElement(MessageView, { role: 'assistant', text: '```js\nconst answer = 42;\n```' }));
  assert.equal((html.match(/<pre>/g) || []).length, 1);
  assert.equal((html.match(/class="codeblock"/g) || []).length, 1);
  assert.match(html, /Copy js block/);
});

test('chat file inspections are collapsed while the actual answer stays visible', () => {
  const html = renderToStaticMarkup(createElement(MessageView, {
    role: 'assistant', text: '```tool\n{"name":"read_file","args":{"path":"README.md"}}\n```RageV is a game engine.',
    activities: [
      {kind:'tool_result',state:'OBSERVING',iteration:1,tool:'read_file',args:{path:'README.md'},output:'# RageV',message:'Inspection finished'},
      {kind:'response',state:'COMPLETED',iteration:2,message:'RageV is a game engine.'},
    ],
  }));
  assert.match(html, /<details class="chat-file-activity"><summary>View file activity<\/summary>/);
  assert.match(html, /<\/details><p>RageV is a game engine\.<\/p>/);
  assert.doesNotMatch(html, /structured-message|Open agent activity|&quot;name&quot;/);
});

test('unlabelled code fences stay copyable while inline code stays inline', () => {
  const html = renderToStaticMarkup(createElement(MessageView, { role: 'assistant', text: 'Use `answer`.\n\n```\n<script>not executable</script>\n```' }));
  assert.match(html, /<code>answer<\/code>/);
  assert.match(html, /Copy code block/);
  assert.equal((html.match(/<pre>/g) || []).length, 1);
  assert.doesNotMatch(html, /<script>/);
  assert.match(html, /&lt;script&gt;/);
});
