import { Children, isValidElement, useState } from 'react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import type { AgentEvent } from '../services/api';
import ToolTimeline from './ToolTimeline';
import CodeBlock from './CodeBlock';
import ResponseMetrics from './ResponseMetrics';
import type { OutputTiming } from '../services/outputTiming';

function renderers() {
  return {
    // Render at the fence wrapper, not at <code>: otherwise the rich code
    // block becomes an invalid nested <pre>. Unlabelled fences get Copy too.
    pre(props: any) {
      const child = Children.toArray(props.children)[0];
      if (!isValidElement(child)) return <pre>{props.children}</pre>;
      const childProps = child.props as { className?: string; children?: unknown };
      const lang = /(?:^|\s)language-(\S+)/.exec(childProps.className ?? '')?.[1] ?? '';
      return <CodeBlock lang={lang} code={String(childProps.children ?? '').replace(/\n$/, '')} />;
    },
  };
}

function fmtTime(iso: string): string {
  if (!iso) return '';
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return '';
  return d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
}

// Design guide §8: search sources render as compact chips, not a text dump.
function splitSources(text: string): { body: string; sources: { title: string; url: string }[] } {
  const m = text.match(/\n\nSources\n([\s\S]*)$/);
  if (!m || m.index == null) return { body: text, sources: [] };
  const sources: { title: string; url: string }[] = [];
  for (const line of m[1].split('\n')) {
    const s = line.match(/^\d+\.\s+(.*?)\s+-\s+(https?:\/\/\S+)\s*$/);
    if (s) sources.push({ title: s[1], url: s[2] });
  }
  if (sources.length === 0) return { body: text, sources: [] };
  return { body: text.slice(0, m.index), sources };
}

/** Legacy display only: a model's tool-shaped text is a request, not proof
 * that the action ran. New messages carry a separate execution journal. */
function parseAgentTranscript(text: string, streaming = false): AgentEvent[] | null {
  if (!text.includes('```tool')) return null;
  const blocks: { type: 'text' | 'tool'; value: string }[] = [];
  const re = /```tool\s*([\s\S]*?)```/g;
  let cursor = 0;
  let match: RegExpExecArray | null;
  while ((match = re.exec(text))) {
    const prose = text.slice(cursor, match.index).trim();
    if (prose) blocks.push({ type: 'text', value: prose });
    blocks.push({ type: 'tool', value: match[1].trim() });
    cursor = match.index + match[0].length;
  }
  const tail = text.slice(cursor).trim();
  if (tail) blocks.push({ type: 'text', value: tail });
  if (!blocks.some((block) => block.type === 'tool')) return null;

  let lastText = -1;
  for (let i = blocks.length - 1; i >= 0; i -= 1) {
    if (blocks[i].type === 'text') { lastText = i; break; }
  }
  let iteration = 0;
  return blocks.flatMap((block, index): AgentEvent[] => {
    if (block.type === 'text') {
      const completed = index === lastText && index === blocks.length - 1 && !streaming;
      return [{
        state: 'OBSERVING',
        kind: completed ? 'response' : 'thought',
        message: block.value,
        iteration: Math.max(1, iteration),
      }];
    }
    try {
      const call = JSON.parse(block.value) as { name?: unknown; args?: unknown };
      if (typeof call.name !== 'string') throw new Error('missing tool name');
      iteration += 1;
      const args = call.args && typeof call.args === 'object' ? call.args as Record<string, unknown> : {};
      return [{
        state: 'OBSERVING',
        kind: 'tool_request',
        tool: call.name,
        args,
        message: 'Requested by the model. This older transcript has no recorded execution result.',
        iteration,
      }];
    } catch {
      return [{
        state: 'OBSERVING',
        kind: 'status',
        message: 'The model returned an unreadable action. New runs retry this automatically.',
        iteration: Math.max(1, iteration),
      }];
    }
  });
}

export default function MessageView({
  role,
  text,
  time,
  streaming,
  tps,
  live,
  timing,
  legacyRate,
  showMetrics = true,
  detailedMetrics = false,
  activities,
  agentRunId,
  onOpenAgentActivity,
}: {
  role: 'user' | 'assistant' | 'tool';
  text: string;
  time?: string;
  streaming?: boolean;
  /** Measured output tok/s; null hides the indicator. */
  tps?: number | null;
  live?: boolean;
  timing?: OutputTiming | null;
  legacyRate?: boolean;
  showMetrics?: boolean;
  detailedMetrics?: boolean;
  activities?: AgentEvent[];
  agentRunId?: string;
  onOpenAgentActivity?: (runId: string) => void;
}) {
  const [copied, setCopied] = useState(false);
  const { body, sources } = role === 'assistant' ? splitSources(text) : { body: text, sources: [] as { title: string; url: string }[] };
  const isAgentRun = !!activities?.some((event) => event.kind === 'task');
  const isChatActivity = role === 'assistant' && !!activities?.length && !isAgentRun;
  const agentEvents = role === 'assistant' ? (activities?.length
    ? isAgentRun ? activities : null
    : parseAgentTranscript(body, !!streaming)) : null;
  const chatDetails = isChatActivity ? activities!.filter((event) => event.kind !== 'response') : [];
  const chatResponses = isChatActivity ? activities!.filter((event) => event.kind === 'response').map((event) => event.message).join('\n\n') : '';
  const answerBody = isChatActivity
    ? chatResponses || (streaming ? body.replace(/```tool\s*[\s\S]*?(?:```|$)/g, '').trim() : '')
    : body;
  if (role === 'user') {
    return (
      <div className="msg user">
        {text}
        <div className="meta">
          {time && <span>{fmtTime(time)}</span>}
          <span className="msg-actions" style={{ marginTop: 0 }}>
            <button onClick={() => { void navigator.clipboard.writeText(text).then(() => { setCopied(true); setTimeout(() => setCopied(false), 1500); }); }}>
              {copied ? 'Copied' : 'Copy'}
            </button>
          </span>
        </div>
      </div>
    );
  }
  if (agentEvents) {
    return (
      <div className="msg assistant structured-message">
        <ToolTimeline events={agentEvents} />
        {sources.length > 0 && (
          <div className="source-chips" style={{ display: 'flex', gap: 6, flexWrap: 'wrap', marginTop: 6 }} aria-label="Sources">
            {sources.map((source, index) => (
              <a key={source.url} className="ui-chip" href={source.url} target="_blank" rel="noreferrer" title={source.url}>
                {index + 1}. {source.title}
              </a>
            ))}
          </div>
        )}
        {isAgentRun && agentRunId && onOpenAgentActivity && (
          <button className="agent-inline-open" onClick={() => onOpenAgentActivity(agentRunId)}>
            Open agent activity
          </button>
        )}
        <div className="meta">
          {time && <span>{fmtTime(time)}</span>}
          {showMetrics && <ResponseMetrics tps={tps} live={live} timing={timing} legacy={legacyRate} detailed={detailedMetrics} />}
          <span className="msg-actions" style={{ marginTop: 0 }}>
            <button onClick={() => { void navigator.clipboard.writeText(text).then(() => { setCopied(true); setTimeout(() => setCopied(false), 1500); }); }}>
              {copied ? 'Copied' : 'Copy'}
            </button>
          </span>
        </div>
      </div>
    );
  }
  return (
    <div className={`msg ${role}`}>
      {chatDetails.length > 0 && <details className="chat-file-activity">
        <summary>View file activity</summary>
        <ToolTimeline events={chatDetails} />
      </details>}
      <ReactMarkdown remarkPlugins={[remarkGfm]} components={renderers()}>
        {answerBody || (streaming ? '…' : '')}
      </ReactMarkdown>
      {streaming && <span className="caret" aria-label="generating">▍</span>}
      {sources.length > 0 && (
        <div style={{ display: 'flex', gap: 6, flexWrap: 'wrap', marginTop: 6 }} aria-label="Sources">
          {sources.map((s, i) => (
            <a key={i} className="ui-chip" href={s.url} target="_blank" rel="noreferrer" title={s.url}>
              <span>{i + 1}. {s.title}</span>
            </a>
          ))}
        </div>
      )}
      {(!streaming || tps != null) && (text || streaming) && (
        <div className="meta">
          {time && <span>{fmtTime(time)}</span>}
          {showMetrics && <ResponseMetrics tps={tps} live={live} timing={timing} legacy={legacyRate} detailed={detailedMetrics} />}
          <span className="msg-actions" style={{ marginTop: 0 }}>
            <button onClick={() => { void navigator.clipboard.writeText(text).then(() => { setCopied(true); setTimeout(() => setCopied(false), 1500); }); }}>
              {copied ? 'Copied' : 'Copy'}
            </button>
          </span>
        </div>
      )}
    </div>
  );
}
