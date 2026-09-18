import test from 'node:test';
import assert from 'node:assert/strict';
import { autoTitle, matchesShortcut, nextPermissionMode, PermissionModeSaver, planAwaitingApproval, selectAvailableModel, shouldStartAgent, updateMessage, validatedPanelWidth, WORKBENCH_DESTINATIONS } from './workbench.ts';

test('new chats and new code tasks are titled from their first message', () => {
  assert.equal(autoTitle('New task', 'What does this project do?'), 'What does this project do?');
  assert.equal(autoTitle('New chat', '/plan Add a total_quantity function'), 'Add a total_quantity function');
  assert.equal(autoTitle('My renamed session', 'anything'), null, 'a real title stays');
  assert.equal(autoTitle(undefined, 'anything'), null);
  assert.equal(autoTitle('New task', '   '), null);
  const long = autoTitle('New task', 'Create a to-do list web app in a new folder named todo: a single index.html');
  assert.equal(long, 'Create a to-do list web app in a new folder…');
  assert.ok(long.length <= 49);
});

test('utility navigation keeps each destination once, with Settings last', () => {
  const ids = WORKBENCH_DESTINATIONS.map(({id}) => id);
  assert.deepEqual(ids, ['models', 'resources', 'system', 'tools', 'settings']);
  assert.equal(new Set(ids).size, ids.length);
});

test('startup honors the installed default without replacing a loaded or manually selected model', () => {
  const models = [{id:'model-a'}, {id:'model-b'}];
  assert.equal(selectAvailableModel(models, '', 'model-b'), 'model-b');
  assert.equal(selectAvailableModel(models, 'model-a', 'model-b'), 'model-a');
  assert.equal(selectAvailableModel([{id:'model-a',loaded:true}, {id:'model-b'}], '', 'model-b'), 'model-a');
  assert.equal(selectAvailableModel(models, 'missing', 'also-missing'), 'model-a');
  assert.equal(selectAvailableModel([], '', 'model-b'), '');
});

test('Ask mode never launches a writing agent', () => {
  assert.equal(shouldStartAgent('code', 'looks good to me'), true, 'the agent answers acknowledgements itself');
  assert.equal(shouldStartAgent('code', 'find the pending tasks'), true);
  assert.equal(shouldStartAgent('code', 'Implement the calculator'), true);
  assert.equal(shouldStartAgent('chat', 'hello'), false);
  assert.equal(shouldStartAgent('code', '/test'), false);
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

test('Shift+Tab cycles Ask, Accept edits and Plan, and never passes through Auto', () => {
  assert.equal(nextPermissionMode('ask'), 'accept_edits');
  assert.equal(nextPermissionMode('accept_edits'), 'plan');
  assert.equal(nextPermissionMode('plan'), 'ask');
  assert.equal(nextPermissionMode('auto'), 'ask');
  // A read-only model (its tool check) leaves only Ask to cycle to.
  assert.equal(nextPermissionMode('ask', ['ask']), 'ask');
  assert.equal(nextPermissionMode('auto', ['ask']), 'ask');
  assert.equal(nextPermissionMode('ask', ['ask', 'plan']), 'plan');
  assert.equal(nextPermissionMode('unknown'), 'ask');
});

test('mode saves run one at a time, save only where cycling stops, and report failures', async () => {
  const saves = [];
  const outcomes = [];
  let fail = false;
  let release;
  const saver = new PermissionModeSaver('ask', (mode) => {
    saves.push(mode);
    if (fail) return Promise.reject(new Error('offline'));
    return new Promise((resolve) => { release = () => resolve({ mode, resumed: 0 }); });
  }, (outcome) => outcomes.push(outcome.error ? `failed:${outcome.mode}` : outcome.mode));
  const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  // Cycling through two modes quickly saves only the last.
  saver.schedule('accept_edits', 20);
  saver.schedule('plan', 20);
  await wait(40);
  assert.deepEqual(saves, ['plan']);
  release();
  await wait(0);
  assert.deepEqual(outcomes, ['plan']);
  // A request made while a save is out waits for it, then saves the latest.
  const first = saver.request('accept_edits');
  await wait(0);
  const second = saver.request('ask');
  await wait(0);
  assert.deepEqual(saves, ['plan', 'accept_edits'], 'one save at a time');
  release();
  await wait(0);
  assert.deepEqual(saves, ['plan', 'accept_edits', 'ask']);
  release();
  assert.equal(await first, true);
  assert.equal(await second, true);
  assert.equal(saver.mode, 'ask');
  assert.deepEqual(outcomes, ['plan', 'ask'], 'only the latest wanted mode is reported');
  // A failure puts the shown mode back to the server's.
  fail = true;
  assert.equal(await saver.request('auto'), false);
  assert.equal(saver.mode, 'ask');
  assert.deepEqual(outcomes, ['plan', 'ask', 'failed:ask']);
  // settle() saves a scheduled mode at once.
  fail = false;
  saver.schedule('plan', 10_000);
  const settled = saver.settle();
  await wait(0);
  release();
  assert.equal(await settled, true);
  assert.equal(saver.mode, 'plan');
});

test('only the latest completed, unanswered plan run that presented a plan waits for approval', () => {
  const plan = { id: 'p', state: 'COMPLETED', mode: 'plan', plan_ready: true };
  assert.equal(planAwaitingApproval([plan], new Set()), 'p');
  assert.equal(planAwaitingApproval([{ ...plan, plan_ready: false }], new Set()), null, 'a plan run that answered a question');
  assert.equal(planAwaitingApproval([{ id: 'p', state: 'COMPLETED', mode: 'plan' }], new Set()), null, 'older backends');
  assert.equal(planAwaitingApproval([plan], new Set(['p'])), null, 'answered');
  assert.equal(planAwaitingApproval([plan, { id: 'a', state: 'COMPLETED', mode: 'agent' }], new Set()), null, 'a later run');
  assert.equal(planAwaitingApproval([{ ...plan, state: 'EXECUTING_TOOL' }], new Set()), null, 'still running');
  assert.equal(planAwaitingApproval([{ ...plan, state: 'FAILED' }], new Set()), null, 'failed');
  assert.equal(planAwaitingApproval([], new Set()), null);
});
