import test from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';
import { build } from 'esbuild';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';

const result = await build({ entryPoints: [fileURLToPath(new URL('../src/components/WorkStatus.tsx', import.meta.url))], bundle:true, write:false, platform:'node', format:'cjs', jsx:'automatic', external:['react','react/*','react-dom','react-dom/*'] });
const loaded = {exports:{}};
new Function('require','module','exports',result.outputFiles[0].text)(createRequire(import.meta.url),loaded,loaded.exports);
const render = (props) => renderToStaticMarkup(createElement(loaded.exports.default,props));
test('idle status renders nothing rather than pretending to work', () => {
  assert.equal(render({active:false,label:'Working'}),'');
});
test('active status announces actual phase and keeps the timer out of live announcements', () => {
  const html = render({active:true,label:'Companion is writing…',startedAt:Date.now()});
  assert.match(html,/work-status live/);
  assert.match(html,/role="status".*Companion is writing…/);
  assert.match(html,/work-status-time.*aria-hidden="true"/);
  assert.match(html,/>0s<\/span>/);
});
test('restored activity immediately renders its existing duration rather than zero', () => {
  const html = render({ active: true, label: 'Working…', startedAt: Date.now() - 151000 });
  assert.match(html, />2m 31s<\/span>/);
  assert.match(html, /including waits/);
});
test('an unknown start never fabricates elapsed time from component mount', () => {
  const html = render({ active: true, label: 'Working…' });
  assert.match(html, /Activity start time unavailable/);
  assert.match(html, />—<\/span>/);
});
test('approval waits do not display the working animation', () => {
  const html = render({active:true,label:'Waiting for your approval',waiting:true});
  assert.match(html,/work-status waiting/);
  assert.doesNotMatch(html,/work-status live/);
});
