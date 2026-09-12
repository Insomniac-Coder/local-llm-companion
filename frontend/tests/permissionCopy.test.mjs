import test from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';
import { readFileSync } from 'node:fs';
import { build } from 'esbuild';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';

const result = await build({ entryPoints: [fileURLToPath(new URL('../src/components/PermissionsModal.tsx', import.meta.url))], bundle: true, write: false, platform: 'node', format: 'cjs', jsx: 'automatic', external: ['react', 'react/*'] });
const loaded = { exports: {} };
new Function('require', 'module', 'exports', result.outputFiles[0].text)(createRequire(import.meta.url), loaded, loaded.exports);

test('permission summary clearly describes full Auto while preserving the actual saved policy', () => {
  for (const enabled of [true, false]) {
    const html = renderToStaticMarkup(createElement(loaded.exports.PermissionSummary, { settings: { agent: { autonomous_enabled: enabled }, search: { autonomous: 'deny' } } }));
    assert.ok(html.includes(enabled ? 'Auto — no approval prompts' : 'Ask — approve actions'));
    for (const text of ['file edits, commands and deletions', 'not operating-system sandboxed', 'request’s Search switch', 'overrides Auto', 'Not allowed']) assert.ok(html.includes(text), text);
    assert.match(html, /Ask mode requests approval for agent actions/);
    assert.doesNotMatch(html, /commands and deletion still require|Network policy:|Safe project inspection does not need approval/);
  }
});

test('active permission surfaces no longer advertise file-only Auto or automatic command approval prompts', () => {
  for (const path of ['App.tsx', 'components/SettingsPanel.tsx', 'components/PermissionsModal.tsx']) {
    const text = readFileSync(new URL(`../src/${path}`, import.meta.url), 'utf8');
    assert.doesNotMatch(text, /Auto files|Auto permits bounded file edits|Commands and deletions still require approval|even when automatic file edits are on/);
  }
});

test('Ask and Auto expose and visibly style their selected state', () => {
  const app = readFileSync(new URL('../src/App.tsx', import.meta.url), 'utf8');
  const css = readFileSync(new URL('../src/workbench.css', import.meta.url), 'utf8');
  assert.match(app, /aria-pressed=\{permissionMode === 'ask'\}/);
  assert.match(app, /aria-pressed=\{permissionMode === 'auto'\}/);
  assert.match(css, /\.permission-mode button\[aria-pressed='true'\]/);
  assert.match(css, /\.permission-mode\.auto button\[aria-pressed='true'\]/);
});
