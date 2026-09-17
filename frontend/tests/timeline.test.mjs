import test from 'node:test';
import assert from 'node:assert/strict';
import { stepLabel, visibleTimelineEvents } from '../src/services/timeline.ts';

const event = (fields) => ({ state: 'PLANNING', message: '', iteration: 1, ...fields });

test('a final answer the last progress note repeats is shown once', () => {
  const answer = 'Based on the code in `project/orders.py`: it reads an order from a JSON file and prints the total including tax.';
  const rows = visibleTimelineEvents([
    event({ kind: 'task', iteration: 0, message: 'What does this project do?' }),
    event({ kind: 'tool_started', tool: 'read_file', iteration: 4 }),
    event({ kind: 'tool_result', tool: 'read_file', iteration: 4, state: 'OBSERVING' }),
    event({ kind: 'thought', iteration: 5, message: 'Reading the tax code first.' }),
    event({ kind: 'thought', iteration: 5, message: answer }),
    event({ kind: 'context', iteration: 5 }),
    event({ kind: 'final', iteration: 5, state: 'COMPLETED', message: answer }),
  ]);
  assert.deepEqual(rows.map((row) => row.kind), ['task', 'tool_result', 'thought', 'final']);
  assert.equal(rows[2].message, 'Reading the tax code first.', 'other progress stays');
  // A long answer's progress copy is cut at 1,200 characters: a prefix is a repeat too.
  const long = 'x'.repeat(1500);
  const cut = visibleTimelineEvents([event({ kind: 'thought', message: long.slice(0, 1200) }), event({ kind: 'final', state: 'COMPLETED', message: long })]);
  assert.deepEqual(cut.map((row) => row.kind), ['final']);
  // A short note that merely starts the answer is not hidden.
  const short = visibleTimelineEvents([event({ kind: 'thought', message: 'Done.' }), event({ kind: 'final', state: 'COMPLETED', message: 'Done. The page is ready.' })]);
  assert.deepEqual(short.map((row) => row.kind), ['thought', 'final']);
});

test('only the latest generic "preparing" row is kept; informative status rows stay', () => {
  const rows = visibleTimelineEvents([
    event({ kind: 'status', message: 'Preparing the first action…' }),
    event({ kind: 'tool_result', tool: 'list_directory', state: 'OBSERVING' }),
    event({ kind: 'status', iteration: 2, message: 'Reviewing the result and preparing the next action…' }),
    event({ kind: 'status', iteration: 2, message: 'The model returned an incomplete or unreadable action. Retrying…' }),
    event({ kind: 'status', iteration: 3, message: 'Reviewing the result and preparing the next action…' }),
  ]);
  assert.deepEqual(rows.map((row) => row.message), ['', 'The model returned an incomplete or unreadable action. Retrying…', 'Reviewing the result and preparing the next action…']);
});

test('rows are labelled with the model step they belong to', () => {
  assert.equal(stepLabel(event({ iteration: 5 })), 'Step 5');
  assert.equal(stepLabel(event({ iteration: 0 })), '');
});

test('a tool started in a run that ended without its result says so', () => {
  const rows = visibleTimelineEvents([
    event({ kind: 'tool_started', tool: 'write_file', iteration: 2 }),
    event({ kind: 'error', iteration: 2, state: 'FAILED', message: 'Stopped.' }),
  ]);
  assert.equal(rows[0].kind, 'tool_error');
  assert.match(rows[0].message, /No result was recorded/);
});
