import test from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';
import { build } from 'esbuild';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { startScriptsFor } from '../src/services/startScripts.ts';

const bundled = await build({ entryPoints: [fileURLToPath(new URL('../src/components/StartScripts.tsx', import.meta.url))], bundle: true, write: false, platform: 'node', format: 'cjs', jsx: 'automatic', external: ['react', 'react/*'] });
const loaded = { exports: {} };
new Function('require', 'module', 'exports', bundled.outputFiles[0].text)(createRequire(import.meta.url), loaded, loaded.exports);
const StartScripts = loaded.exports.default;

const WINDOWS_FIRST = [['.\\run.ps1', 'run.bat'], ['./run.sh']];
const UNIX_FIRST = [['./run.sh'], ['.\\run.ps1', 'run.bat']];

test('both start scripts are named, this computer first, for every value browsers report', () => {
  for (const platform of ['Windows', 'Win32', '']) {
    assert.deepEqual(startScriptsFor(platform).map(({ commands }) => commands), WINDOWS_FIRST, JSON.stringify(platform));
  }
  for (const platform of ['macOS', 'MacIntel', 'Linux', 'Linux x86_64', 'Linux armv8l', 'Chrome OS', 'Chromium OS']) {
    assert.deepEqual(startScriptsFor(platform).map(({ commands }) => commands), UNIX_FIRST, platform);
  }
});

test('the restart message renders both scripts with their systems, in this browser\'s order', () => {
  const original = Object.getOwnPropertyDescriptor(globalThis, 'navigator');
  const render = (navigator) => {
    Object.defineProperty(globalThis, 'navigator', { value: navigator, configurable: true, writable: true });
    return renderToStaticMarkup(createElement(StartScripts));
  };
  try {
    assert.equal(
      render({ userAgentData: { platform: 'Windows' }, platform: 'Win32' }),
      '<code>.\\run.ps1</code> or <code>run.bat</code> on Windows, or <code>./run.sh</code> on macOS and Linux',
    );
    assert.equal(
      render({ userAgentData: { platform: 'Chrome OS' }, platform: 'Linux x86_64' }),
      '<code>./run.sh</code> on macOS and Linux, or <code>.\\run.ps1</code> or <code>run.bat</code> on Windows',
    );
    // Safari and Firefox have no userAgentData.
    assert.equal(
      render({ platform: 'MacIntel' }),
      '<code>./run.sh</code> on macOS and Linux, or <code>.\\run.ps1</code> or <code>run.bat</code> on Windows',
    );
  } finally {
    if (original) Object.defineProperty(globalThis, 'navigator', original);
    else delete globalThis.navigator;
  }
});
