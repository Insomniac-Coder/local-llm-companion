import { useState } from 'react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import type { AgentEvent } from '../services/api';

type ActivityKind = 'task' | 'thought' | 'read' | 'list' | 'search' | 'edit' | 'write' | 'delete' | 'command' | 'permission' | 'final' | 'error' | 'tool' | 'status' | 'request' | 'response';

const META: Record<ActivityKind, { label: string; icon: string }> = {
  task: { label: 'Task', icon: '◇' },
  thought: { label: 'Progress', icon: '✦' },
  read: { label: 'Read file', icon: '↳' },
  list: { label: 'Listed files', icon: '≡' },
  search: { label: 'Search', icon: '⌕' },
  edit: { label: 'Modified file', icon: '±' },
  write: { label: 'Wrote file', icon: '+' },
  delete: { label: 'Deleted file', icon: '−' },
  command: { label: 'Command', icon: '›_' },
  permission: { label: 'Permission', icon: '!' },
  final: { label: 'Completed', icon: '✓' },
  error: { label: 'Needs attention', icon: '×' },
  tool: { label: 'Tool', icon: '◆' },
  status: { label: 'Status', icon: '·' },
  request: { label: 'Requested action', icon: '?' },
  response: { label: 'Response', icon: '·' },
};

function classify(e: AgentEvent): ActivityKind {
  if (e.state === 'FAILED' || e.state === 'CANCELLED' || e.kind === 'tool_error') return 'error';
  if (e.kind === 'tool_request') return 'request';
  if (e.kind === 'response') return 'response';
  if (e.kind === 'task') return 'task';
  if (e.kind === 'thought') return 'thought';
  if (e.kind === 'final' || e.state === 'COMPLETED') return 'final';
  if (e.kind === 'permission' || e.state === 'WAITING_PERMISSION') return 'permission';
  if (e.tool === 'read_file') return 'read';
  if (e.tool === 'list_directory') return 'list';
  if (e.tool === 'search_text' || e.tool === 'web_search') return 'search';
  if (e.tool === 'edit_file') return 'edit';
  if (e.tool === 'write_file' || e.tool === 'create_document') return 'write';
  if (e.tool === 'delete_file') return 'delete';
  if (e.tool === 'execute_command' || e.tool === 'git_commit') return 'command';
  if (e.kind === 'tool_started' || e.kind === 'tool_result' || e.state === 'EXECUTING_TOOL') return 'tool';
  if (e.state === 'PLANNING') return 'thought';
  return 'status';
}

function arg(e: AgentEvent, name: string): string {
  const value = e.args?.[name];
  return typeof value === 'string' ? value : '';
}

function actionTitle(e: AgentEvent, kind: ActivityKind): string {
  if (!e.tool) return META[kind].label;
  const target = arg(e, 'path') || arg(e, 'query') || arg(e, 'command');
  return target || e.tool.replace(/_/g, ' ');
}

function Diff({ value }: { value: string }) {
  return (
    <pre className="activity-diff" aria-label="File changes">
      {value.split('\n').map((line, i) => {
        const tone = line.startsWith('+++') || line.startsWith('---')
          ? 'file'
          : line.startsWith('+')
            ? 'add'
            : line.startsWith('-')
              ? 'remove'
              : line.startsWith('@@')
                ? 'hunk'
                : '';
        return <span key={i} className={tone}>{line || ' '}{'\n'}</span>;
      })}
    </pre>
  );
}

function ActivityItem({ event, index }: { event: AgentEvent; index: number }) {
  const kind = classify(event);
  const meta = META[kind];
  const isRunning = event.kind === 'tool_started';
  const narrative = kind === 'task' || kind === 'thought' || kind === 'final' || (kind === 'error' && !event.tool) || kind === 'status' || kind === 'response';
  const hasDetails = !!(event.diff || event.output || event.args || event.pending_tool);
  const [expanded, setExpanded] = useState(!!event.diff);

  return (
    <article className={`activity-item ${kind}${isRunning ? ' running' : ''}`}>
      <div className="activity-node" aria-hidden="true"><span>{meta.icon}</span></div>
      <div className="activity-content">
        <div className="activity-titlebar">
          <div className="activity-heading">
            <span className="activity-label">{meta.label}</span>
            {!narrative && <strong title={actionTitle(event, kind)}>{actionTitle(event, kind)}</strong>}
          </div>
          <span className="activity-step">{isRunning ? 'Running' : `Step ${index + 1}`}</span>
        </div>

        {narrative && (
          <div className={`activity-copy ${kind}`}>
            <ReactMarkdown remarkPlugins={[remarkGfm]}>{event.message}</ReactMarkdown>
          </div>
        )}

        {!narrative && event.message && event.message !== actionTitle(event, kind) && (
          <div className="activity-summary">{event.message}</div>
        )}

        {hasDetails && (
          <>
            <button className="activity-disclosure" onClick={() => setExpanded((v) => !v)} aria-expanded={expanded}>
              <span aria-hidden="true">{expanded ? '⌄' : '›'}</span>
              {event.diff ? 'View changes' : event.output ? 'View output' : 'View details'}
            </button>
            {expanded && (
              <div className="activity-details">
                {event.diff && <Diff value={event.diff} />}
                {!event.diff && event.output && <pre className="activity-output">{event.output}</pre>}
                {!event.diff && !event.output && event.pending_tool && (
                  <div className="activity-permission-reason">{event.pending_tool.reason}</div>
                )}
                {!event.diff && !event.output && !event.pending_tool && event.args && (
                  <pre className="activity-output">{JSON.stringify(event.args, null, 2)}</pre>
                )}
              </div>
            )}
          </>
        )}
      </div>
    </article>
  );
}

export default function ToolTimeline({ events }: { events: AgentEvent[] }) {
  if (events.length === 0) return null;
  const terminal = events.some((event) => ['COMPLETED', 'FAILED', 'CANCELLED'].includes(event.state));
  const visible = events.reduce<AgentEvent[]>((items, event) => {
    if (event.kind === 'context') return items;
    if (event.kind === 'tool_result' || event.kind === 'tool_error') {
      const pending = items.findIndex((item) => item.kind === 'tool_started'
        && item.iteration === event.iteration && item.tool === event.tool);
      if (pending >= 0) { items[pending] = event; return items; }
    }
    items.push(event);
    return items;
  }, []).map((event): AgentEvent => terminal && event.kind === 'tool_started' ? {
    ...event, kind: 'tool_error', state: 'OBSERVING', diff: undefined,
    message: 'No result was recorded before this run ended. Inspect the current file or output before retrying.',
  } : event);
  return (
    <div className="activity-stream" aria-label="Agent activity timeline">
      {visible.map((event, i) => (
        <ActivityItem key={`${event.iteration}-${event.kind ?? event.state}-${i}`} event={event} index={i} />
      ))}
    </div>
  );
}
