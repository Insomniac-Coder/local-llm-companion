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

test('permission summary names the saved mode and describes all four, including full Auto', () => {
  const cases = [
    [{ permission_mode: 'ask' }, 'Ask — Reads and searches run freely'],
    [{ permission_mode: 'accept_edits' }, 'Accept edits — Reads and file edits in the project run without asking'],
    [{ permission_mode: 'plan' }, 'Plan — Read-only'],
    [{ permission_mode: 'auto', autonomous_enabled: true }, 'Auto — Auto carries out your requested task'],
    // A settings file from before the modes keeps meaning what its switch said.
    [{ autonomous_enabled: true }, 'Auto — Auto carries out your requested task'],
    [{ autonomous_enabled: false }, 'Ask — Reads and searches run freely'],
  ];
  for (const [agent, current] of cases) {
    const html = renderToStaticMarkup(createElement(loaded.exports.PermissionSummary, { settings: { agent, search: { autonomous: 'deny' } } }));
    assert.ok(html.includes(current), `${JSON.stringify(agent)}: ${current}`);
    for (const text of ['file edits, commands and deletions', 'not operating-system sandboxed', 'request’s Search switch', 'overrides Auto', 'Not allowed', 'Accept edits: ', 'Plan: ']) assert.ok(html.includes(text), text);
    assert.doesNotMatch(html, /commands and deletion still require|Network policy:|Safe project inspection does not need approval/);
  }
});

test('active permission surfaces no longer advertise file-only Auto or automatic command approval prompts', () => {
  for (const path of ['App.tsx', 'components/SettingsPanel.tsx', 'components/PermissionsModal.tsx']) {
    const text = readFileSync(new URL(`../src/${path}`, import.meta.url), 'utf8');
    assert.doesNotMatch(text, /Auto files|Auto permits bounded file edits|Commands and deletions still require approval|even when automatic file edits are on/);
  }
});

test('the permission modes expose and visibly style their selected state', () => {
  const app = readFileSync(new URL('../src/App.tsx', import.meta.url), 'utf8');
  const css = readFileSync(new URL('../src/workbench.css', import.meta.url), 'utf8');
  assert.match(app, /PERMISSION_MODES\.map\(\(option\) =>/);
  assert.match(app, /aria-pressed=\{permissionMode === option\}/);
  assert.match(css, /\.permission-mode button\[aria-pressed='true'\]/);
  assert.match(css, /\.permission-mode\.auto button\[aria-pressed='true'\]/);
});
