import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';
import { build } from 'esbuild';
import ts from 'typescript';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';

const sourcePath = fileURLToPath(new URL('../src/components/SettingsPanel.tsx', import.meta.url));
const result = await build({ entryPoints: [sourcePath], bundle: true, write: false, platform: 'node', format: 'cjs', jsx: 'automatic', external: ['react', 'react/*'], loader: { '.css': 'empty' } });
const loaded = { exports: {} };
new Function('require', 'module', 'exports', result.outputFiles[0].text)(createRequire(import.meta.url), loaded, loaded.exports);
const { SettingField, HardwareOverrides, RuntimeSummary } = loaded.exports;

test('loaded runtime exposes CPU fallback explanation rather than only next-load settings', () => {
  const cpu = { architecture: 'test', weights_quantization: 'Q4', effective_context: 8192, cache_type_k: 'f16', cache_type_v: 'f16', threads: 0, gpu_layers: 0, batch_size: 128, flash_attention: 'off', kv_offload: 'off', notes: ['CPU mode selected automatically: no usable GPU.'] };
  const html = renderToStaticMarkup(createElement(RuntimeSummary, { policy: { active: cpu, next: { ...cpu, notes: [] } }, dirty: false }));
  assert.match(html, /Loaded session[\s\S]*CPU mode selected automatically: no usable GPU/);
  assert.match(html, /GPU layers<\/dt><dd>0<\/dd>/);
});

test('optional Auto badge stays inside the same control row and the label remains linked', () => {
  const html = renderToStaticMarkup(createElement(SettingField, { label: 'GPU layers (-1 auto)' }, createElement('input', { type: 'number', defaultValue: -1 }), createElement('span', null, 'Auto')));
  const labelId = /<label[^>]*for="([^"]+)"/.exec(html)?.[1];
  assert.ok(labelId);
  assert.ok(html.includes(`id="${labelId}"`));
  assert.match(html, /class="settings-field-control"><input[^>]+><span>Auto<\/span><\/div>/);
});

test('checkbox settings have their own row and clickable native label', () => {
  const html = renderToStaticMarkup(createElement(SettingField, { label: 'Flash attention' }, createElement('input', { type: 'checkbox', defaultChecked: true })));
  assert.match(html, /settings-field--toggle/);
  const id = /for="([^"]+)"/.exec(html)?.[1];
  assert.ok(id && html.includes(`id="${id}"`));
  assert.match(html, /checked=""/);
});

test('default-model select, refresh action and help stay in one accessible field', () => {
  const html = renderToStaticMarkup(createElement(SettingField, { label: 'Default model', description: 'Changes apply after Save.' },
    createElement('select', { defaultValue: 'missing-id' }, createElement('option', { value: '' }, 'Automatic selection'), createElement('option', { value: 'missing-id' }, 'Previously selected model unavailable')),
    createElement('button', { type: 'button' }, 'Refresh')));
  const id = /<label[^>]*for="([^"]+)"/.exec(html)?.[1];
  assert.ok(id && html.includes(`id="${id}"`));
  assert.ok(html.includes(`aria-describedby="${id}-description"`));
  assert.ok(html.includes(`id="${id}-description"`));
  assert.match(html, /<option value="missing-id" selected="">Previously selected model unavailable/);
  assert.match(html, /<\/select><button type="button">Refresh<\/button><\/div>/);
});

test('every settings section contains explicit field rows, including conditional fields', () => {
  const source = ts.createSourceFile(sourcePath, readFileSync(sourcePath, 'utf8'), ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  let sectionCount = 0;
  function visit(node) {
    if (ts.isJsxElement(node) && node.openingElement.attributes.properties.some((attribute) => ts.isJsxAttribute(attribute) && attribute.name.getText(source) === 'className' && attribute.initializer?.text === 'settings-fields')) {
      sectionCount++;
      for (const child of node.children) {
        if (ts.isJsxText(child) && !child.text.trim()) continue;
        const field = ts.isJsxExpression(child) && child.expression && ts.isBinaryExpression(child.expression) ? child.expression.right : child;
        assert.ok(ts.isJsxElement(field) && field.openingElement.tagName.getText(source) === 'SettingField', 'all fields must be contained in one row');
      }
    }
    ts.forEachChild(node, visit);
  }
  visit(source);
  assert.equal(sectionCount, 6);
});

test('hardware overrides are hidden by default and return with preserved values only in manual mode', () => {
  const settings = { hardware: { cpu_threads: 7, gpu_layers: -1, flash_attention: true, kv_cache_gpu: true }, inference: { batch_size: 256 } };
  assert.equal(renderToStaticMarkup(createElement(HardwareOverrides, { settings, set() {} })), '');
  assert.equal(renderToStaticMarkup(createElement(HardwareOverrides, { settings: { ...settings, runtime_auto: true }, set() {} })), '');
  const html = renderToStaticMarkup(createElement(HardwareOverrides, { settings: { ...settings, runtime_auto: false }, set() {} }));
  assert.match(html, /CPU threads/);
  assert.match(html, /value="7"/);
  assert.match(html, /Prompt batch size/);
  assert.match(html, /value="256"/);
});

test('runtime details separate requested configuration from measurements and unsaved changes', () => {
  const resolved = { mode: 'automatic', architecture: 'test', weights_quantization: 'Q4_K_M', effective_context: 32768, cache_type_k: 'f16', cache_type_v: 'f16', threads: 0, gpu_layers: -1, batch_size: 512, flash_attention: 'auto', kv_offload: 'auto', notes: [] };
  const policy = { running: true, model_name: 'Loaded model', active: resolved, next: { ...resolved, effective_context: 8192 } };
  const html = renderToStaticMarkup(createElement(RuntimeSummary, { policy, dirty: true }));
  assert.match(html, /not measurements of memory use/);
  assert.match(html, /Runtime managed/);
  assert.match(html, /Save your changes/);
  assert.doesNotMatch(html, /8,192/);
  const saved = renderToStaticMarkup(createElement(RuntimeSummary, { policy, dirty: false }));
  assert.match(saved, /8,192/);
});

test('inactive preferences are not exposed as editable settings', () => {
  const source = readFileSync(sourcePath, 'utf8');
  for (const key of ['allowed_dirs', 'blocked_dirs', 'auto_compact', 'max_attach_mb', 'max_image_mb', 'output_dir', 'log_redaction', 'telemetry', 'server_port', 'kv_cache_type', 'log_level', 'confirm_outside_copy', 'default_dir', 'command_timeout_secs']) {
    assert.equal(source.includes(`'${key}'`), false, `${key} must not be editable`);
  }
  assert.match(source, /Older, inactive preferences remain/);
  assert.match(source, /saved custom provider is not implemented/);
});
