import { memo, useEffect, useRef, useState } from 'react';

/** Keep the highlighter out of initial navigation and avoid re-highlighting on
 * every streamed token. Plain text stays visible while a grammar is loading. */
const CodeBlock = memo(function CodeBlock({ lang, code }: { lang: string; code: string }) {
  const [copyState, setCopyState] = useState<'idle' | 'copied' | 'error'>('idle');
  const [highlighted, setHighlighted] = useState<{ code: string; html: string } | null>(null);
  const copyTimer = useRef<ReturnType<typeof setTimeout>>();

  useEffect(() => () => clearTimeout(copyTimer.current), []);
  useEffect(() => {
    let cancelled = false;
    // Large logs/diffs remain complete and copyable without blocking the UI.
    if (!lang || code.length > 120_000) return;
    const timer = setTimeout(() => {
      void import('./syntaxHighlighter').then(({ highlightCode }) => highlightCode(code, lang))
        .then((html) => { if (!cancelled) setHighlighted({ code, html }); })
        .catch(() => { /* Network/grammar failure leaves safe, readable plain text. */ });
    }, 120);
    return () => { cancelled = true; clearTimeout(timer); };
  }, [lang, code]);

  const copy = async () => {
    clearTimeout(copyTimer.current);
    try { await navigator.clipboard.writeText(code); setCopyState('copied'); }
    catch { setCopyState('error'); }
    copyTimer.current = setTimeout(() => setCopyState('idle'), 2000);
  };
  return (
    <div className="codeblock">
      <div className="codeblock-bar">
        <span>{lang || 'code'}</span>
        <button onClick={() => void copy()} aria-label={`Copy ${lang || 'code'} block`}>
          {copyState === 'copied' ? 'Copied' : copyState === 'error' ? 'Copy failed — retry' : 'Copy'}
        </button>
      </div>
      <pre>{highlighted?.code === code
        ? <code dangerouslySetInnerHTML={{ __html: highlighted.html }} />
        : <code>{code}</code>}</pre>
    </div>
  );
});

export default CodeBlock;
