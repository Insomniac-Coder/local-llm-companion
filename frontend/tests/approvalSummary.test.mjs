import test from 'node:test';
import assert from 'node:assert/strict';
import { approvalTarget } from '../src/services/approvalSummary.ts';

test('the approval card names what will run or change, not just the tool', () => {
  assert.equal(approvalTarget('execute_command', { command: 'python -m unittest', cwd: 'project' }), 'python -m unittest\n(in project)');
  assert.equal(approvalTarget('execute_command', { command: 'npm test', cwd: '.' }), 'npm test');
  assert.equal(approvalTarget('delete_file', { path: 'old/notes.txt' }), 'old/notes.txt');
  assert.equal(approvalTarget('write_file', { path: 'index.html', content: '<html>' }), 'index.html');
  assert.equal(approvalTarget('git_commit', { message: 'Add low_stock' }), 'Add low_stock');
  assert.equal(approvalTarget('web_search', { query: 'llama.cpp gemma 4' }), 'llama.cpp gemma 4');
  assert.equal(approvalTarget('execute_command', null), '');
});
