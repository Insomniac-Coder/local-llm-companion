import test from 'node:test';
import assert from 'node:assert/strict';
import { currentActivitySnapshot, elapsedSeconds, elapsedLabel, parseActivityStart, visibleWorkActivity } from '../src/services/workElapsed.ts';

test('elapsed time continues across view unmounts, tab waits and fresh mounts', () => {
  const started = parseActivityStart('2026-09-11T12:00:00Z');
  assert.equal(elapsedLabel(elapsedSeconds(started, started + 151000)), '2m 31s');
  assert.equal(elapsedLabel(elapsedSeconds(started, started + 241000)), '4m 1s');
  assert.equal(elapsedSeconds(started + 300000, started + 305000), 5, 'a new request has its own start');
});

test('visible activity always takes its kind and clock from the same session', () => {
  const chat = { conversationId: 'chat-a', startedAt: 1000 };
  const agent = { conversationId: 'code-b', startedAt: 2000 };
  assert.deepEqual(visibleWorkActivity('chat-a', true, chat, true, agent), { kind: 'chat', startedAt: 1000 });
  assert.deepEqual(visibleWorkActivity('code-b', true, chat, true, agent), { kind: 'agent', startedAt: 2000 });
  assert.equal(visibleWorkActivity('other', true, chat, true, agent), null);
  assert.equal(visibleWorkActivity(null, true, chat, true, agent), null);
  assert.equal(visibleWorkActivity('code-b', true, chat, false, agent), null);
});

test('restoring a legacy run never substitutes a chat clock for its unknown start', () => {
  const chat = { conversationId: 'code-a', startedAt: 1000 };
  const agent = { conversationId: 'code-a', startedAt: parseActivityStart(undefined) };
  assert.deepEqual(visibleWorkActivity('code-a', true, chat, true, agent), { kind: 'agent', startedAt: null });
  assert.equal(elapsedLabel(visibleWorkActivity('code-a', true, chat, true, agent).startedAt), '—');
});

test('stale polls and absent pre-start summaries cannot replace the active lifecycle', () => {
  assert.equal(currentActivitySnapshot('a', 'b', 1, 1, false), false);
  assert.equal(currentActivitySnapshot('a', 'a', 1, 2, false), false);
  assert.equal(currentActivitySnapshot('a', 'a', 2, 2, true), false);
  assert.equal(currentActivitySnapshot('a', 'a', 2, 2, false), true);
});

test('legacy or invalid starts remain unknown and clock skew never goes negative', () => {
  for (const value of [undefined, null, '', 'invalid']) assert.equal(parseActivityStart(value), null);
  assert.equal(elapsedSeconds(null, 1000), null);
  assert.equal(elapsedSeconds(Number.NaN, 1000), null);
  assert.equal(elapsedSeconds(2000, 1000), 0);
  assert.equal(elapsedLabel(null), '—');
});
