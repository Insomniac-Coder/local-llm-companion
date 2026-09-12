import test from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';
import { build } from 'esbuild';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';

const compiled = await build({entryPoints:[fileURLToPath(new URL('../src/components/ResponseMetrics.tsx', import.meta.url))],bundle:true,write:false,platform:'node',format:'cjs',jsx:'automatic',external:['react','react/*','react-dom','react-dom/*']});
const loaded = {exports:{}};
new Function('require','module','exports',compiled.outputFiles[0].text)(createRequire(import.meta.url),loaded,loaded.exports);
const render = (props) => renderToStaticMarkup(createElement(loaded.exports.default,props));
const timing = {basis:'visible_output_v1',output_tps:24.5,estimated:false,token_basis:'tokenizer',output_tokens:49,first_visible_ms:8000,output_ms:2000,thinking_ms:null,total_ms:10000};

test('first text latency and output speed are separately labeled, without invented thinking duration', () => {
  const html = render({timing});
  assert.match(html,/First text 8.0s/);
  assert.match(html,/Output 24.5 tok\/s/);
  assert.doesNotMatch(html,/>Thinking /);
});
test('estimates and model-reported thinking are explicit', () => {
  const html = render({timing:{...timing,estimated:true,thinking_ms:2500}});
  assert.match(html,/Output ≈24.5 tok\/s/);
  assert.match(html,/Thinking 2.5s/);
  assert.match(render({live:true,tps:12}),/Output ≈12.0 tok\/s/);
});
test('old inclusive rates and buffered output are not mislabeled as measured output speed', () => {
  assert.match(render({legacy:true,tps:10}),/Legacy overall 10.0 tok\/s/);
  assert.equal(render({live:true,tps:null}),'');
  assert.match(render({timing:{...timing,output_tps:null}}),/Output rate —/);
});

test('detailed metrics preserve the distinction between emission time and total turn time', () => {
  const html = render({timing,detailed:true});
  assert.match(html,/Output time 2.0s/);
  assert.match(html,/Total 10.0s/);
  assert.doesNotMatch(render({timing}),/Output time /);
});
