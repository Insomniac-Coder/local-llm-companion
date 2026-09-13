import { Children, isValidElement, useEffect, useRef, useState } from 'react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import type { AgentEvent } from '../services/api';
import ToolTimeline from './ToolTimeline';
import CodeBlock from './CodeBlock';
import ResponseMetrics from './ResponseMetrics';
import type { OutputTiming } from '../services/outputTiming';
import { Button, IconButton } from '../ui/primitives';
import { Icon } from '../ui/Icon';

// Defined once. Passing freshly created renderer functions on every render
// makes React treat them as new component types, so each background refresh
// would unmount every code block and flash it uncoloured before re-highlighting.
const MARKDOWN_COMPONENTS = {
  // Render at the fence wrapper, not at <code>: otherwise the rich code
  // block becomes an invalid nested <pre>. Unlabelled fences get Copy too.
  pre(props: any) {
    const child = Children.toArray(props.children)[0];
    if (!isValidElement(child)) return <pre>{props.children}</pre>;
    const childProps = child.props as { className?: string; children?: unknown };
    const lang = /(?:^|\s)language-(\S+)/.exec(childProps.className ?? '')?.[1] ?? '';
    return <CodeBlock lang={lang} code={String(childProps.children ?? '').replace(/\n$/, '')} />;
  },
  // Links open outside the app so a click never navigates the workbench away.
  a(props: any) {
    return <a href={props.href} target="_blank" rel="noreferrer">{props.children}</a>;
  },
};

const REMARK_PLUGINS = [remarkGfm];

function fmtTime(iso: string): string {
  if (!iso) return '';
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return '';
  return d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
}

// Search sources render as compact chips, not a text dump.
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

function CopyAction({ text }: { text: string }) {
  const [copied, setCopied] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout>>();
  useEffect(() => () => clearTimeout(timer.current), []);
  return (
    <IconButton
      icon={copied ? 'check' : 'copy'}
      label={copied ? 'Copied' : 'Copy message'}
      size="sm"
      tipSide="top"
      onClick={() => {
        void navigator.clipboard.writeText(text).then(() => {
          setCopied(true);
          clearTimeout(timer.current);
          timer.current = setTimeout(() => setCopied(false), 1500);
        });
      }}
    />
  );
}

function Sources({ sources }: { sources: { title: string; url: string }[] }) {
  if (sources.length === 0) return null;
  return (
    <div className="source-chips" aria-label="Sources">
      {sources.map((source, index) => (
        <a key={`${index}-${source.url}`} className="chip" href={source.url} target="_blank" rel="noreferrer" title={source.url}>
          <Icon name="globe" size={13} />
          <span>{index + 1}. {source.title}</span>
        </a>
      ))}
    </div>
  );
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
  byline,
  thinking,
  onOpenAgentActivity,
  onEdit,
  onRegenerate,
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
  /** Model that produced this reply, when recorded. */
  byline?: string;
  /** Native reasoning streamed for this reply during the session (not persisted). */
  thinking?: string;
  onOpenAgentActivity?: (runId: string) => void;
  onEdit?: () => void;
  onRegenerate?: () => void;
}) {
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
      <>
        <div className="msg user">{text}</div>
        <div className="meta">
          <span className="msg-actions">
            {onEdit && <IconButton icon="pencil" label="Edit message" size="sm" tipSide="top" onClick={onEdit} />}
            <CopyAction text={text} />
          </span>
          {time && <span>{fmtTime(time)}</span>}
        </div>
      </>
    );
  }

  if (role === 'tool') {
    return (
      <div className="msg tool" role="note">
        <Icon name="alertCircle" size={16} />
        <span>{text}</span>
      </div>
    );
  }

  const meta = (
    <div className="meta">
      {byline && <span className="byline">{byline}</span>}
      {time && <span>{fmtTime(time)}</span>}
      {showMetrics && <ResponseMetrics tps={tps} live={live} timing={timing} legacy={legacyRate} detailed={detailedMetrics} />}
      <span className="msg-actions">
        {onRegenerate && <IconButton icon="refresh" label="Regenerate reply" size="sm" tipSide="top" onClick={onRegenerate} />}
        <CopyAction text={text} />
      </span>
    </div>
  );

  if (agentEvents) {
    return (
      <div className="msg assistant structured-message">
        <ToolTimeline events={agentEvents} />
        <Sources sources={sources} />
        {isAgentRun && agentRunId && onOpenAgentActivity && (
          <Button size="sm" variant="ghost" iconRight="arrowRight" className="agent-inline-open" onClick={() => onOpenAgentActivity(agentRunId)}>
            Open agent activity
          </Button>
        )}
        {meta}
      </div>
    );
  }

  return (
    <div className={`msg ${role}${streaming ? ' streaming' : ''}`}>
      {chatDetails.length > 0 && <details className="chat-file-activity">
        <summary>View file activity</summary>
        <ToolTimeline events={chatDetails} />
      </details>}
      {thinking && (
        <details className="thinking-block" open={streaming && !answerBody}>
          <summary>{streaming && !answerBody ? 'Thinking…' : 'Thought process'}</summary>
          <pre>{thinking}</pre>
        </details>
      )}
      <ReactMarkdown remarkPlugins={REMARK_PLUGINS} components={MARKDOWN_COMPONENTS}>
        {answerBody}
      </ReactMarkdown>
      {streaming && <span className="live-rule" role="status" aria-label="Generating" />}
      <Sources sources={sources} />
      {(!streaming || tps != null) && (text || streaming) && meta}
    </div>
  );
}
