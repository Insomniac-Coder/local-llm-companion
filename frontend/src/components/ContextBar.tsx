import { useEffect, useRef, useState } from 'react';
import { healthLabel } from '../services/events';
import { contextDisplay } from '../services/contextUsage';
import type { ContextInfo } from '../services/api';
import './contextbar.css';

const ORDER: { key: keyof NonNullable<ContextInfo['breakdown']>; label: string }[] = [
  { key: 'conversation', label: 'Messages and summary' },
  { key: 'attachments', label: 'Included attachment text' },
  { key: 'tools', label: 'Saved tool replies' },
  { key: 'memory', label: 'Included memory' },
];
const fmtK = (value: number) => value >= 1000 ? `${(value / 1000).toFixed(1)}K` : `${value}`;

export default function ContextBar({ ctx, onCompact, compacting }: {
  ctx: ContextInfo | null;
  onCompact: () => void;
  compacting: boolean;
}) {
  const [open, setOpen] = useState(false);
  const container = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const outside = (event: PointerEvent) => { if (!container.current?.contains(event.target as Node)) setOpen(false); };
    document.addEventListener('pointerdown', outside);
    return () => document.removeEventListener('pointerdown', outside);
  }, [open]);
  if (!ctx || (!ctx.limit && !ctx.agent_context?.usage?.context_limit)) return null;
  const display = contextDisplay(ctx);
  const agent = ctx.agent_context;
  const usage = agent?.usage;
  const compactDisabled = compacting || !!agent?.active;
  const compactTip = agent?.active ? 'Wait for the run to finish. Saved-history compaction does not change an active agent transcript.' : 'Summarize saved context for the next request; original history is preserved.';
  return (
    <div className="contextbar detailed" ref={container} onKeyDown={(event) => { if (event.key === 'Escape') setOpen(false); }}>
      <button className="ctx-toggle" onClick={() => setOpen((value) => !value)} title="Inspect saved context and the latest agent input" aria-expanded={open}>
        {display.label} {display.pending ? '· preparing…' : <>{display.estimated ? '≈' : ''}{fmtK(display.tokens!)} / {fmtK(display.limit)} <span className={`ctx-bar ${display.health === 'critical' ? 'crit' : display.health === 'high' ? 'warn' : ''}`}><span style={{ width: `${display.percent}%` }} /></span> {display.percentLabel}</>}
        {!display.pending && display.health !== 'healthy' ? ` · ${healthLabel(display.health)}` : ''}
      </button>
      {(display.health === 'high' || display.health === 'critical') && <button onClick={onCompact} disabled={compactDisabled} title={compactTip}>{compacting ? 'Compacting…' : 'Compact saved context'}</button>}
      {open && <div className="ctx-inspector" role="dialog" aria-label="Context breakdown">
        <header className="ctx-inspector-heading"><strong>What the model can see</strong><button onClick={() => setOpen(false)} aria-label="Close context breakdown">×</button></header>
        {usage ? <section className="ctx-agent-input">
          <div className="ctx-row total"><span>{agent?.active ? 'Agent task input' : 'Last agent task input'}</span><strong>{display.estimated ? '≈' : ''}{fmtK(display.tokens!)} tokens</strong></div>
          <p>{display.estimated ? usage.phase === 'request' ? 'Estimated from the assembled request. Runtime token usage is not available yet.' : 'The runtime did not return prompt usage, so this is a text-based estimate.' : 'Prompt tokens reported by the model runtime for one request, not a running total.'}</p>
          <div className="ctx-row"><span>Input turns · iteration {agent?.iteration}</span><span>{usage.turns}</span></div>
          <div className="ctx-row"><span>Output budget for this request</span><span>{fmtK(usage.output_reserve)}</span></div>
          {usage.generated_tokens != null && <div className="ctx-row"><span>Reported output tokens</span><span>{fmtK(usage.generated_tokens)}</span></div>}
          {usage.pruned_turns > 0 && <div className="ctx-row"><span>Older turns pruned during this run</span><span>{usage.pruned_turns}</span></div>}
          {usage.context_limit !== ctx.limit && <p>This recorded request used a {fmtK(usage.context_limit)}-token window, shown in the meter. The currently configured window for new requests is {fmtK(ctx.limit)} tokens.</p>}
          {usage.images > 0 && <p>{usage.images} image{usage.images === 1 ? '' : 's'} included. Text estimates do not include image token costs.</p>}
          <p>Includes the instructions, carried conversation and tool results still present in that agent request. Reading a file adds its returned text to the next request; it does not load the whole project. Separate completion checks are not included.</p>
        </section> : <p className="ctx-explanation">{agent?.active ? 'The agent is preparing its input. Its first request measurement will appear here.' : 'This is saved conversation context, not a measurement of live model input. Older runs may not have recorded runtime usage.'}</p>}
        <section className="ctx-saved-input">
          <div className="ctx-row total"><strong>Saved context estimate</strong><strong>≈{fmtK(ctx.estimated_tokens)}</strong></div>
          {ctx.breakdown && ORDER.map(({ key, label }) => <div key={key} className="ctx-row"><span>{label}</span><span>{fmtK(ctx.breakdown![key])}</span></div>)}
          <p>Bounded saved text for a future request. Runtime instructions and the agent’s temporary working transcript are counted separately above. Original conversation history stays saved.</p>
        </section>
        <footer className="ctx-inspector-footer"><button onClick={onCompact} disabled={compactDisabled} title={compactTip}>{compacting ? 'Compacting…' : 'Compact saved context'}</button><span>{agent?.active ? 'Available after this run' : 'Keeps original history'}</span></footer>
      </div>}
    </div>
  );
}
