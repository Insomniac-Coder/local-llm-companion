import hljs from 'highlight.js/lib/core';
import type { LanguageFn } from 'highlight.js';

// Only load the grammar the user is looking at. Uncommon languages retain the
// previous full-language support through an on-demand fallback.
const loaders: Record<string, () => Promise<{ default: LanguageFn }>> = {
  javascript: () => import('highlight.js/lib/languages/javascript'),
  typescript: () => import('highlight.js/lib/languages/typescript'),
  python: () => import('highlight.js/lib/languages/python'),
  rust: () => import('highlight.js/lib/languages/rust'),
  cpp: () => import('highlight.js/lib/languages/cpp'),
  c: () => import('highlight.js/lib/languages/c'),
  csharp: () => import('highlight.js/lib/languages/csharp'),
  java: () => import('highlight.js/lib/languages/java'),
  kotlin: () => import('highlight.js/lib/languages/kotlin'),
  go: () => import('highlight.js/lib/languages/go'),
  bash: () => import('highlight.js/lib/languages/bash'),
  powershell: () => import('highlight.js/lib/languages/powershell'),
  json: () => import('highlight.js/lib/languages/json'),
  yaml: () => import('highlight.js/lib/languages/yaml'),
  css: () => import('highlight.js/lib/languages/css'),
  xml: () => import('highlight.js/lib/languages/xml'),
  sql: () => import('highlight.js/lib/languages/sql'),
  diff: () => import('highlight.js/lib/languages/diff'),
  markdown: () => import('highlight.js/lib/languages/markdown'),
};
const aliases: Record<string, string> = {
  js: 'javascript', jsx: 'javascript', ts: 'typescript', tsx: 'typescript', py: 'python',
  rs: 'rust', 'c++': 'cpp', cs: 'csharp', 'c#': 'csharp', kt: 'kotlin', golang: 'go',
  sh: 'bash', shell: 'bash', zsh: 'bash', ps1: 'powershell', yml: 'yaml',
  html: 'xml', svg: 'xml', md: 'markdown', patch: 'diff',
};
const pending = new Map<string, Promise<void>>();

export async function highlightCode(code: string, requested: string): Promise<string> {
  const name = requested.toLowerCase();
  const language = aliases[name] || name;
  if (loaders[language]) {
    if (!pending.has(language)) {
      pending.set(language, loaders[language]().then((grammar) => hljs.registerLanguage(language, grammar.default))
        .catch((error) => { pending.delete(language); throw error; }));
    }
    await pending.get(language);
    return hljs.highlight(code, { language, ignoreIllegals: true }).value;
  }
  if (['text', 'plaintext', 'txt', 'console', 'output'].includes(language)) return escapeCode(code);
  const full = (await import('highlight.js')).default;
  return full.getLanguage(language) ? full.highlight(code, { language, ignoreIllegals: true }).value : escapeCode(code);
}

function escapeCode(code: string) {
  return code.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
}
