import test from 'node:test';
import assert from 'node:assert/strict';
import { classifyRequest, startAgent, streamChat } from './api.ts';
import { shouldStartAgent } from './workbench.ts';

test('routing uses only the validated model label and sends the exact message with its session', async () => {
  const originalFetch = globalThis.fetch;
  const calls = [];
  const controller = new AbortController();
  let reply;
  globalThis.fetch = async (url, init) => {
    calls.push({url, init});
    return new Response(JSON.stringify(reply), {status: 200});
  };
  try {
    const message = 'tell me more about rageV project';
    for (const label of ['ask', 'plan', 'agent']) {
      reply = {intent: label, source: 'model'};
      const result = await classifyRequest(message, 'session', controller.signal);
      assert.equal(result.intent, label);
      assert.equal(shouldStartAgent('code', result.intent, message), label !== 'ask');
    }
    for (const invalid of [null, {intent:'inspect'}, {intent:'agent',source:'untrusted'}]) {
      reply = invalid;
      assert.deepEqual(await classifyRequest(message, 'session', controller.signal), {intent:'ask',source:'fallback'});
    }
    assert.ok(calls.every(({url, init}) => url === '/api/chat/classify' && init.signal === controller.signal));
    assert.deepEqual(JSON.parse(calls[0].init.body), {message, conversation_id:'session'});
  } finally { globalThis.fetch = originalFetch; }
});

test('agent start preserves conversational dispositions and forwards explicit search consent', async () => {
  const originalFetch = globalThis.fetch;
  const calls = [];
  const replies = [
    { disposition: 'conversation', message: 'Glad that helped.', message_id: 'ack-1' },
    { disposition: 'run', run_id: 'task-1', state: 'PLANNING' },
    { disposition: 'needs_task', message: 'What should I work on?', message_id: 'clarify-1' },
  ];
  globalThis.fetch = async (url, init) => {
    calls.push({url, body: JSON.parse(init.body)});
    return new Response(JSON.stringify(replies.shift()), {status: 200});
  };
  try {
    const acknowledgement = await startAgent('project', 'looks good to me', 'agent', 'session');
    assert.equal(acknowledgement.disposition, 'conversation');
    assert.equal('run_id' in acknowledgement, false);
    const run = await startAgent('project', 'Check current docs', 'plan', 'session', {search: true});
    assert.equal(run.run_id, 'task-1');
    const clarification = await startAgent('project', 'continue', 'agent', 'session');
    assert.equal(clarification.disposition, 'needs_task');
    assert.equal('run_id' in clarification, false);
    assert.deepEqual(calls.map(({body}) => body.search), [false, true, false]);
    assert.ok(calls.every(({url, body}) => url === '/api/agent/run' && body.workspace === 'project' && body.conversation_id === 'session'));
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test('stream separates reported phases from visible Unicode output and final timing', async () => {
  const originalFetch = globalThis.fetch;
  const phases = [], tokens = [];
  let usage;
  const timing = {basis:'visible_output_v1',output_tps:12,estimated:false,token_basis:'tokenizer',output_tokens:12,first_visible_ms:8000,output_ms:1000,thinking_ms:3000,total_ms:9000};
  const bytes = new TextEncoder().encode(`event: phase\ndata: {"phase":"processing","round":1}\n\nevent: phase\ndata: {"phase":"thinking","round":1}\n\nevent: phase\ndata: {"phase":"responding","round":1}\n\nevent: token\ndata: Hello 🌎\n\nevent: token\ndata:  again\n\nevent: done\ndata: ${JSON.stringify({message_id:'reply',prompt_tokens:50,generated_tokens:99,timing})}\n\n`);
  globalThis.fetch = async () => new Response(new ReadableStream({start(controller) { for (const byte of bytes) controller.enqueue(Uint8Array.of(byte)); controller.close(); }}));
  try {
    await streamChat('hello','session',{onToken:token=>tokens.push(token),onPhase:event=>phases.push(event.phase),onDone:value=>usage=value},new AbortController().signal);
    assert.deepEqual(phases,['processing','thinking','responding']);
    assert.equal(tokens.join(''),'Hello 🌎 again');
    assert.equal(usage.message_id,'reply');
    assert.deepEqual(usage.timing,timing);
  } finally { globalThis.fetch = originalFetch; }
});
