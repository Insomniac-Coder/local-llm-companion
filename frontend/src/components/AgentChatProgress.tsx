import { useEffect, useMemo, useRef, useState } from 'react';
import {
  agentRuns,
  streamAgentEvents,
  type AgentEvent,
  type AgentRunSummary,
} from '../services/api';
import { Lamp } from '../ui/primitives';
import { Icon } from '../ui/Icon';

const TERMINAL = ['COMPLETED', 'FAILED', 'CANCELLED'];

function mergeEvent(previous: AgentEvent[], event: AgentEvent) {
  if (event.kind === 'tool_result' || event.kind === 'tool_error') {
    let index = -1;
    for (let i = previous.length - 1; i >= 0; i -= 1) {
      if (previous[i].kind === 'tool_started' && previous[i].iteration === event.iteration && previous[i].tool === event.tool) {
        index = i;
        break;
      }
    }
    if (index >= 0) {
      const next = [...previous];
      next[index] = event;
      return next;
    }
  }
  return [...previous, event];
}

function arg(event: AgentEvent, key: string) {
  const value = event.args?.[key];
  return typeof value === 'string' ? value : '';
}

function compactTarget(value: string, max = 64) {
  const clean = value.trim().replace(/\\/g, '/');
  return clean.length > max ? `…${clean.slice(-(max - 1))}` : clean;
}

function commandUpdate(command: string) {
  const lower = command.toLowerCase();
  const directory = command.match(/(?:mkdir|new-item\s+(?:-itemtype\s+directory\s+)?)(?:\s+-\w+\s+\S+)*\s+["']?([^"';&|]+)["']?/i)?.[1]?.trim();
  if (/\b(mkdir|new-item)\b/.test(lower)) return directory ? `Created directory ${compactTarget(directory)}` : 'Created a directory';
  if (/\b(test|pytest|vitest|jest|cargo test)\b/.test(lower)) return 'Ran the tests';
  if (/\b(build|cargo check|tsc|lint)\b/.test(lower)) return 'Checked the build';
  return `Ran ${compactTarget(command, 54)}`;
}

function updateLabel(event: AgentEvent) {
  if (event.kind === 'tool_error') return `Action failed: ${event.message.slice(0, 100)}`;
  if (event.kind === 'tool_started') return `Running: ${event.message.slice(0, 100)}`;
  const target = compactTarget(arg(event, 'path') || arg(event, 'query'));
  if (event.kind === 'task') return 'Agent started';
  if (event.kind === 'permission') return 'Waiting for your approval';
  if (event.kind === 'final') return 'Finished the task';
  if (event.kind === 'error') return event.message.trim().slice(0, 110) || 'Reviewed the partial work';
  if (event.kind === 'status') {
    if (/checking the work/i.test(event.message)) return 'Checking the result against your request';
    if (/remaining work|continuing/i.test(event.message)) return 'Found more work and continued automatically';
    if (/step failed|another approach/i.test(event.message)) return 'A step failed — evaluating another approach';
    if (/unreadable action/i.test(event.message)) return 'Corrected an unreadable model action';
    return event.message.replace(/…/g, '').trim().slice(0, 100);
  }
  if (event.kind === 'thought') return event.message.trim().split('\n')[0].slice(0, 100);
  if (event.kind !== 'tool_started' && event.kind !== 'tool_result') return '';

  switch (event.tool) {
    case 'list_directory': return target ? `Checked ${target}` : 'Checked the project files';
    case 'read_file': return target ? `Read ${target}` : 'Read a file';
    case 'search_text': return target ? `Searched for ${target}` : 'Searched the project';
    case 'write_file': return target ? `Created ${target}` : 'Created a file';
    case 'edit_file': return target ? `Updated ${target}` : 'Updated a file';
    case 'delete_file': return target ? `Removed ${target}` : 'Removed a file';
    case 'execute_command': return commandUpdate(arg(event, 'command'));
    case 'create_document': return target ? `Created ${target}` : 'Created a document';
    case 'web_search': return target ? `Searched the web for ${target}` : 'Searched the web';
    default: return event.message.trim().slice(0, 100);
  }
}

function phaseLabel(state: string | undefined, event: AgentEvent | undefined) {
  if (state === 'COMPLETED') return 'Work completed';
  if (state === 'FAILED') return 'Work needs attention';
  if (state === 'CANCELLED') return 'Work stopped';
  if (state === 'WAITING_PERMISSION') return 'Waiting for approval';
  if (event?.kind === 'status' && /checking|verif/i.test(event.message)) return 'Checking…';
  if (state === 'EXECUTING_TOOL') {
    const command = arg(event ?? { state: '', message: '', iteration: 0 }, 'command').toLowerCase();
    if (/test|pytest|vitest|jest/.test(command)) return 'Testing…';
    if (/build|check|lint|tsc/.test(command)) return 'Checking…';
    return 'Working…';
  }
  if (state === 'OBSERVING') return 'Checking…';
  return 'Thinking…';
}

export default function AgentChatProgress({
  convId,
  focusRun,
  runtimeRunning = true,
  onContextUsage,
  onOpenActivity,
  onFinished,
}: {
  convId: string | null;
  focusRun?: string | null;
  runtimeRunning?: boolean;
  onContextUsage?: (event: AgentEvent, runId: string) => void;
  onOpenActivity: (runId: string) => void;
  onFinished: () => void;
}) {
  const [runs, setRuns] = useState<AgentRunSummary[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(focusRun ?? null);
  const [events, setEvents] = useState<AgentEvent[]>([]);
  // Partial model text for the step in progress; cleared by the next journaled event.
  const [liveText, setLiveText] = useState('');
  const finished = useRef(new Set<string>());
  const finishedCallback = useRef(onFinished);
  const contextCallback = useRef(onContextUsage);

  useEffect(() => { finishedCallback.current = onFinished; }, [onFinished]);
  useEffect(() => { contextCallback.current = onContextUsage; }, [onContextUsage]);

  useEffect(() => {
    let mounted = true;
    const refresh = () => agentRuns().then((all) => {
      if (!mounted) return;
      const contextual = all.filter((run) => run.conversation_id === convId);
      setRuns(contextual);
      const requested = focusRun && contextual.some((run) => run.id === focusRun) ? focusRun : null;
      const live = [...contextual].reverse().find((run) => !TERMINAL.includes(run.state));
      const latest = contextual[contextual.length - 1];
      setSelectedId((current) => requested ?? live?.id ?? (contextual.some((run) => run.id === current) ? current : latest?.id ?? null));
    }).catch(() => {});
    void refresh();
    const timer = window.setInterval(refresh, 1800);
    return () => { mounted = false; window.clearInterval(timer); };
  }, [convId, focusRun]);

  useEffect(() => {
    setEvents([]);
    setLiveText('');
    if (!selectedId) return;
    const controller = new AbortController();
    void streamAgentEvents(selectedId, (event) => {
      if (event.kind === 'thought_delta') {
        setLiveText((text) => text + event.message);
        return;
      }
      setLiveText('');
      contextCallback.current?.(event, selectedId);
      setEvents((previous) => mergeEvent(previous, event));
      if (TERMINAL.includes(event.state) && !finished.current.has(selectedId)) {
        finished.current.add(selectedId);
        finishedCallback.current();
      }
    }, controller.signal).catch((error) => {
      if (error?.name !== 'AbortError') console.warn('Could not follow agent activity', error);
    });
    return () => controller.abort();
  }, [selectedId]);

  const selected = runs.find((run) => run.id === selectedId)
    ?? (focusRun ? { id: focusRun, task: 'Starting the requested work', state: 'PLANNING', iterations: 0, conversation_id: convId ?? '' } : undefined);
  const latest = events[events.length - 1];
  const state = latest?.state ?? selected?.state;
  const updates = useMemo(() => {
    const seen = new Set<string>();
    const items: string[] = [];
    for (const event of events) {
      const label = updateLabel(event);
      if (!label || seen.has(label)) continue;
      seen.add(label);
      items.push(label);
    }
    return items.slice(-5);
  }, [events]);

  if (!convId || !selected) return null;
  const active = !TERMINAL.includes(state ?? '');
  // Finished runs are rendered from their durable assistant message, directly
  // after the user request that launched them. Keeping this live-only prevents
  // an old warning or completion card from being duplicated at chat bottom.
  if (!active) return null;
  const unavailable = active && !runtimeRunning;
  const waiting = active && (unavailable || state === 'WAITING_PERMISSION');

  return (
    <section className={`agent-run ${waiting ? 'waiting' : 'live'}`} aria-live="polite" aria-label="Agent progress">
      <header className="agent-run-head">
        <Lamp state={waiting ? 'caution' : 'live'} pulse />
        <div>
          <strong>{unavailable ? 'Model runtime unavailable' : phaseLabel(state, latest)}</strong>
          <span>{unavailable ? 'This run has not completed, but no model is generating. Stop it before restarting the model.' : state === 'WAITING_PERMISSION' ? 'Paused until you approve or deny the requested action.' : state === 'FAILED' && latest?.message ? latest.message : active ? 'Working on the requested task in this project' : selected.task}</span>
        </div>
        {selected.iterations > 0 && <span className="readout">{selected.iterations} step{selected.iterations === 1 ? '' : 's'}</span>}
      </header>
      {updates.length > 0 && (
        <ol className="agent-run-steps">
          {updates.map((update, index) => <li key={`${index}-${update}`}>{update}</li>)}
        </ol>
      )}
      {liveText.trim() && !waiting && (
        <p className="live-thought" aria-live="off" title="What the model is writing right now">
          {liveText.replace(/```tool[\s\S]*$/, '').trim().slice(-220)}
        </p>
      )}
      <button type="button" className="agent-run-open" onClick={() => onOpenActivity(selected.id)}>
        {state === 'WAITING_PERMISSION' ? 'Review the request' : 'Open agent activity'} <Icon name="arrowRight" size={14} />
      </button>
    </section>
  );
}
