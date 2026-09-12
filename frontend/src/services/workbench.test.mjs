import test from 'node:test';
import assert from 'node:assert/strict';
import { matchesShortcut, selectAvailableModel, shouldStartAgent, updateMessage, validatedPanelWidth, WORKBENCH_DESTINATIONS } from './workbench.ts';

test('utility navigation keeps each destination once, with Settings last', () => {
  const ids = WORKBENCH_DESTINATIONS.map(({id}) => id);
  assert.deepEqual(ids, ['models', 'resources', 'system', 'tools', 'settings']);
  assert.equal(new Set(ids).size, ids.length);
});

test('startup honors the installed default without replacing a loaded or manually selected model', () => {
  const models = [{id:'gemma'}, {id:'qwen'}];
  assert.equal(selectAvailableModel(models, '', 'qwen'), 'qwen');
  assert.equal(selectAvailableModel(models, 'gemma', 'qwen'), 'gemma');
  assert.equal(selectAvailableModel([{id:'gemma',loaded:true}, {id:'qwen'}], '', 'qwen'), 'gemma');
  assert.equal(selectAvailableModel(models, 'missing', 'also-missing'), 'gemma');
  assert.equal(selectAvailableModel([], '', 'qwen'), '');
});

test('Ask mode never launches a writing agent', () => {
  assert.equal(shouldStartAgent('code', 'ask', 'looks good to me'), false);
  assert.equal(shouldStartAgent('code', 'ask', 'find the pending tasks'), false);
  assert.equal(shouldStartAgent('code', 'agent', 'Implement the calculator'), true);
  assert.equal(shouldStartAgent('chat', 'agent', 'hello'), false);
  assert.equal(shouldStartAgent('code', 'agent', '/test'), false);
  assert.equal(shouldStartAgent('code', 'unknown', 'edit this'), false);
});
test('configured shortcuts match modifiers exactly and never capture ordinary typing', () => {
  const event = {key:'K', ctrlKey:true, metaKey:false, altKey:false, shiftKey:false};
  assert.equal(matchesShortcut(event,'ctrl+k'),true);
  assert.equal(matchesShortcut(event,'ctrl+shift+k'),false);
  assert.equal(matchesShortcut({...event,ctrlKey:false},'k'),false);
  assert.equal(matchesShortcut({...event,ctrlKey:false,metaKey:true},'cmd+k'),true);
});
test('stream updates target their own message, not the last message', () => {
  const rows = [{id:'first', text:'a'}, {id:'second', text:'b'}];
  const next = updateMessage(rows, 'first', (row) => ({...row, text:'c'}));
  assert.equal(next[1], rows[1]);
  assert.equal(next[0].text, 'c');
  assert.equal(rows[0].text, 'a');
  assert.deepEqual(updateMessage(rows, 'missing', (row) => ({...row, text:'bad'})), rows);
});
test('saved split widths cannot collapse or overflow the workbench', () => {
  assert.equal(validatedPanelWidth('broken'), 400);
  assert.equal(validatedPanelWidth(null), 400);
  assert.equal(validatedPanelWidth(9999), 640);
  assert.equal(validatedPanelWidth('480'), 480);
});
