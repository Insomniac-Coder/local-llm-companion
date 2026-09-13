import { useEffect, useRef, useState } from 'react';
import { healthLabel } from '../services/events';
import { contextDisplay } from '../services/contextUsage';
import type { ContextInfo } from '../services/api';
import { Button, IconButton } from '../ui/primitives';

const ORDER: { key: keyof NonNullable<ContextInfo['breakdown']>; label: string }[] = [
  { key: 'conversation', label: 'Messages and summary' },
  { key: 'attachments', label: 'Included attachment text' },
  { key: 'tools', label: 'Saved tool replies' },
  { key: 'memory', label: 'Included memory' },
];
const fmtK = (value: number) => value >= 1000 ? `${(value / 1000).toFixed(1)}K` : `${value}`;

type Props = { ctx: ContextInfo | null; onCompact: () => void; compacting: boolean };

function hasLimit(ctx: ContextInfo | null): ctx is ContextInfo {
  return !!ctx && !!(ctx.limit || ctx.agent_context?.usage?.context_limit);
}

/** The explanation of what the model can see: agent input first, then saved history. */
function ContextDetails({ ctx, onCompact, compacting, heading }: Props & { ctx: ContextInfo; heading?: boolean }) {
  const display = contextDisplay(ctx);
  const agent = ctx.agent_context;
  const usage = agent?.usage;
  const compactDisabled = compacting || !!agent?.active;
  const compactTip = agent?.active ? 'Wait for the run to finish. Saved-history compaction does not change an active agent transcript.' : 'Summarize saved context for the next request; original history is preserved.';
  const trackTone = display.health === 'critical' ? ' critical' : display.health === 'high' ? ' high' : '';
  return (
    <>
      {heading !== false && (
        <div className="ctx-headline">
          <strong>{display.pending ? '—' : `${display.estimated ? '≈' : ''}${fmtK(display.tokens!)}`}</strong>
          <span>of {fmtK(display.limit)} tokens · {display.label.toLowerCase()}{display.pending ? ' preparing…' : ` · ${display.percentLabel}`}</span>
        </div>
      )}
      <div className={`ctx-track${trackTone}`} aria-hidden="true"><i style={{ width: `${display.percent}%` }} /></div>
      {!display.pending && display.health !== 'healthy' && <p className="ctx-note">{healthLabel(display.health)}</p>}
      {usage ? (
        <section className="ctx-section">
          <div className="ctx-row total"><span>{agent?.active ? 'Agent task input' : 'Last agent task input'}</span><span>{display.estimated ? '≈' : ''}{fmtK(display.tokens!)} tokens</span></div>
          <p className="ctx-note">{display.estimated ? usage.phase === 'request' ? 'Estimated from the assembled request. Runtime token usage is not available yet.' : 'The runtime did not return prompt usage, so this is a text-based estimate.' : 'Prompt tokens reported by the model runtime for one request, not a running total.'}</p>
          <div className="ctx-row"><span>Input turns · iteration {agent?.iteration}</span><span>{usage.turns}</span></div>
          <div className="ctx-row"><span>Output budget for this request</span><span>{fmtK(usage.output_reserve)}</span></div>
          {usage.generated_tokens != null && <div className="ctx-row"><span>Reported output tokens</span><span>{fmtK(usage.generated_tokens)}</span></div>}
          {usage.pruned_turns > 0 && <div className="ctx-row"><span>Older turns pruned during this run</span><span>{usage.pruned_turns}</span></div>}
          {usage.context_limit !== ctx.limit && <p className="ctx-note">This recorded request used a {fmtK(usage.context_limit)}-token window, shown in the meter. The currently configured window for new requests is {fmtK(ctx.limit)} tokens.</p>}
          {usage.images > 0 && <p className="ctx-note">{usage.images} image{usage.images === 1 ? '' : 's'} included. Text estimates do not include image token costs.</p>}
          <p className="ctx-note">Includes the instructions, carried conversation and tool results still present in that agent request. Reading a file adds its returned text to the next request; it does not load the whole project. Separate completion checks are not included.</p>
        </section>
      ) : (
        <p className="ctx-note">{agent?.active ? 'The agent is preparing its input. Its first request measurement will appear here.' : 'This is saved conversation context, not a measurement of live model input. Older runs may not have recorded runtime usage.'}</p>
      )}
      <section className="ctx-section">
        <div className="ctx-row total"><span>Saved context estimate</span><span>≈{fmtK(ctx.estimated_tokens)}</span></div>
        {ctx.breakdown && ORDER.map(({ key, label }) => <div key={key} className="ctx-row"><span>{label}</span><span>{fmtK(ctx.breakdown![key])}</span></div>)}
        <p className="ctx-note">Bounded saved text for a future request. Runtime instructions and the agent’s temporary working transcript are counted separately above. Original conversation history stays saved.</p>
      </section>
      <footer className="ctx-pop-foot">
        <Button size="sm" icon="layers" onClick={onCompact} disabled={compactDisabled} title={compactTip} loading={compacting}>{compacting ? 'Compacting…' : 'Compact saved context'}</Button>
        <span>{agent?.active ? 'Available after this run' : 'Keeps original history'}</span>
      </footer>
    </>
  );
}

/** Compact ring in the composer; opens the breakdown above it. */
export function ContextGauge({ ctx, onCompact, compacting }: Props) {
  const [open, setOpen] = useState(false);
  const container = useRef<HTMLSpanElement>(null);
  useEffect(() => {
    if (!open) return;
    const outside = (event: PointerEvent) => { if (!container.current?.contains(event.target as Node)) setOpen(false); };
    const escape = (event: KeyboardEvent) => { if (event.key === 'Escape') setOpen(false); };
    document.addEventListener('pointerdown', outside);
    document.addEventListener('keydown', escape);
    return () => { document.removeEventListener('pointerdown', outside); document.removeEventListener('keydown', escape); };
  }, [open]);
  if (!hasLimit(ctx)) return null;
  const display = contextDisplay(ctx);
  // An empty session has nothing to inspect; a hollow 0% ring reads as a spinner.
  if (!display.pending && !display.tokens && !ctx.agent_context?.usage) return null;
  const radius = 6.5;
  const circumference = 2 * Math.PI * radius;
  const label = display.pending ? 'Context preparing' : `${display.label}: ${display.estimated ? 'about ' : ''}${fmtK(display.tokens!)} of ${fmtK(display.limit)} tokens (${display.percentLabel})`;
  return (
    <span className="ctx-anchor" ref={container}>
      <button type="button" className={`ctx-gauge ${display.health}`} aria-expanded={open} aria-label={`${label}. Show what the model can see.`} data-tip={open ? undefined : 'What the model can see'} data-tip-side="top" onClick={() => setOpen((value) => !value)}>
        <svg width="16" height="16" viewBox="0 0 16 16" aria-hidden="true" style={{ transform: 'rotate(-90deg)' }}>
          <circle className="ring-track" cx="8" cy="8" r={radius} fill="none" strokeWidth="2" />
          <circle className="ring-fill" cx="8" cy="8" r={radius} fill="none" strokeWidth="2" strokeLinecap="round" strokeDasharray={circumference} strokeDashoffset={circumference * (1 - Math.max(display.pending ? 0 : 0.02, display.percent / 100))} />
        </svg>
        <span>{display.pending ? '…' : display.percentLabel}</span>
      </button>
      {open && (
        <div className="ctx-pop" role="dialog" aria-label="Context breakdown">
          <header className="ctx-pop-head"><strong>What the model can see</strong><IconButton icon="x" label="Close context breakdown" size="sm" tip={false} onClick={() => setOpen(false)} /></header>
          <ContextDetails ctx={ctx} onCompact={onCompact} compacting={compacting} />
        </div>
      )}
    </span>
  );
}

/** Full-width breakdown for the inspector. */
export default function ContextBar({ ctx, onCompact, compacting }: Props) {
  if (!hasLimit(ctx)) return <p className="help">Context usage appears after the session’s first message.</p>;
  return <div className="ctx-inline"><ContextDetails ctx={ctx} onCompact={onCompact} compacting={compacting} /></div>;
}
