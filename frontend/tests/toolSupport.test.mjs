import test from 'node:test';
import assert from 'node:assert/strict';
import { documentKindsNote, toolSupportLabel } from '../src/services/toolSupport.ts';

test('the runtime report outranks the template guess', () => {
  assert.equal(toolSupportLabel({ tool_calling: true, tool_support_source: 'runtime' }), 'Tools confirmed');
  assert.equal(toolSupportLabel({ tool_calling: false, tool_support_source: 'runtime' }), 'No tool support');
  assert.equal(toolSupportLabel({ tool_calling: true, tool_support_source: 'template' }), 'Tools declared by template');
  assert.equal(toolSupportLabel({ tool_calling: false }), 'Tools unverified');
});

test('only a model confirmed without tools gets the plain-text documents note', () => {
  assert.match(documentKindsNote({ tool_calling: false, tool_support_source: 'runtime' }), /plain text only/);
  assert.equal(documentKindsNote({ tool_calling: true, tool_support_source: 'runtime' }), null);
  assert.equal(documentKindsNote({ tool_calling: false }), null);
});
