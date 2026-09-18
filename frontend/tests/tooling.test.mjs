import test from 'node:test';
import assert from 'node:assert/strict';
import {
  availablePermissionModes, checkLabel, codeSessionsReadOnly, currentTooling, firstLoadNotice, toolingBadge, toolingSummary,
} from '../src/services/tooling.ts';

const profile = (method, can_write) => ({
  version: 1, method, can_write, file_text: can_write ? (method === 'native' ? 'arguments' : 'raw_blocks') : 'none',
  checked_at: '2026-09-17T18:00:00Z', runtime: 'b10809 5266f24da75d', template: 'abc', checks: [], duration_ms: 2100,
});
const MODES = ['ask', 'accept_edits', 'plan', 'auto'];

test('a model is only described by a check made for this runtime and template', () => {
  assert.equal(currentTooling({ name: 'M', tooling: profile('native', true), tooling_state: 'stale' }), null);
  assert.equal(currentTooling({ name: 'M', tooling: null, tooling_state: 'unchecked' }), null);
  assert.equal(currentTooling({ name: 'M', tooling: profile('native', true), tooling_state: 'current' }).method, 'native');
});

test('the badge says what the check found, or that it runs on load', () => {
  assert.deepEqual(toolingBadge({ name: 'M', tooling: profile('native', true), tooling_state: 'current' }), { label: 'Tools: native', tone: 'info' });
  assert.deepEqual(toolingBadge({ name: 'M', tooling: profile('text', true), tooling_state: 'current' }), { label: 'Tools: text format', tone: 'info' });
  assert.deepEqual(toolingBadge({ name: 'M', tooling: profile('native', false), tooling_state: 'current' }), { label: 'Read-only code', tone: 'warn' });
  assert.deepEqual(toolingBadge({ name: 'M', tooling: profile('none', false), tooling_state: 'current' }), { label: 'Read-only code', tone: 'warn' });
  assert.equal(toolingBadge({ name: 'M', tooling_state: 'unchecked' }).label, 'Tools: checked on first load');
  assert.equal(toolingBadge({ name: 'M', tooling: profile('native', true), tooling_state: 'stale' }).label, 'Tools: check again on load');
});

test('a load that will run the check is announced first', () => {
  assert.match(firstLoadNotice({ name: 'Gemma', tooling_state: 'unchecked' }), /first load of Gemma takes a few seconds longer/);
  assert.match(firstLoadNotice({ name: 'Gemma', tooling: profile('native', true), tooling_state: 'stale' }), /checks again/);
  assert.equal(firstLoadNotice({ name: 'Gemma', tooling: profile('native', true), tooling_state: 'current' }), null);
  assert.equal(firstLoadNotice({ name: 'Gemma' }), null, 'no runtime to check with, nothing to announce');
  assert.equal(firstLoadNotice(undefined), null);
});

test('a model whose file text did not arrive intact leaves only Ask', () => {
  const readOnly = { name: 'M', tooling: profile('native', false), tooling_state: 'current' };
  assert.equal(codeSessionsReadOnly(readOnly), true);
  assert.deepEqual(availablePermissionModes(MODES, readOnly), ['ask']);
  const writer = { name: 'M', tooling: profile('text', true), tooling_state: 'current' };
  assert.equal(codeSessionsReadOnly(writer), false);
  assert.deepEqual(availablePermissionModes(MODES, writer), MODES);
  // Unchecked or stale: nothing is known yet, so nothing is taken away.
  assert.deepEqual(availablePermissionModes(MODES, { name: 'M', tooling: profile('none', false), tooling_state: 'stale' }), MODES);
  assert.deepEqual(availablePermissionModes(MODES, null), MODES);
});

test('summaries and check names are plain words', () => {
  assert.equal(toolingSummary(profile('native', true)), 'Calls tools natively through the runtime.');
  assert.match(toolingSummary(profile('text', false)), /read-only/);
  assert.match(toolingSummary(profile('none', false)), /chat and read-only code/);
  assert.equal(checkLabel('native_file_text'), 'File text through the runtime');
  assert.equal(checkLabel('something_new'), 'something_new');
});
