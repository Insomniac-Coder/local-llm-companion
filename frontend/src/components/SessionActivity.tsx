import { useEffect, useRef, useState } from 'react';
import {
  agentRuns,
  resumeAgent,
  stopAgent,
  streamAgentEvents,
  type AgentEvent,
  type AgentRunSummary,
} from '../services/api';
import ToolTimeline from './ToolTimeline';

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

export default function SessionActivity({
  convId,
  focusRun,
  notify,
  onFinished,
  onActiveChange,
}: {
  convId: string | null;
  focusRun?: string | null;
  notify: (kind: 'info' | 'success' | 'warning' | 'error', text: string) => void;
  onFinished: () => void;
  onActiveChange: (active: boolean) => void;
}) {
  const [runs, setRuns] = useState<AgentRunSummary[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(focusRun ?? null);
  const [events, setEvents] = useState<AgentEvent[]>([]);
  const [permissionResolved, setPermissionResolved] = useState(false);
  const abort = useRef<AbortController | null>(null);
  const finishedCallback = useRef(onFinished);
  const activeCallback = useRef(onActiveChange);

  useEffect(() => {
    finishedCallback.current = onFinished;
    activeCallback.current = onActiveChange;
  }, [onFinished, onActiveChange]);

  useEffect(() => {
    let mounted = true;
    const refresh = () => agentRuns().then((all) => {
      if (!mounted) return;
      const contextual = all.filter((run) => run.conversation_id === convId);
      setRuns(contextual);
      const requested = focusRun && contextual.some((run) => run.id === focusRun) ? focusRun : null;
      const live = [...contextual].reverse().find((run) => !TERMINAL.includes(run.state));
      const latest = contextual[contextual.length - 1];
      setSelectedId((current) => requested ?? (contextual.some((run) => run.id === current) ? current : live?.id ?? latest?.id ?? null));
      activeCallback.current(contextual.some((run) => !TERMINAL.includes(run.state)));
    }).catch(() => setRuns([]));
    void refresh();
    const timer = window.setInterval(refresh, 2000);
    return () => { mounted = false; window.clearInterval(timer); };
  }, [convId, focusRun]);

  useEffect(() => {
    abort.current?.abort();
    setEvents([]);
    setPermissionResolved(false);
    if (!selectedId) return;
    const controller = new AbortController();
    abort.current = controller;
    void streamAgentEvents(selectedId, (event) => {
      setEvents((previous) => mergeEvent(previous, event));
      if (event.state === 'WAITING_PERMISSION') setPermissionResolved(false);
      if (TERMINAL.includes(event.state)) {
        activeCallback.current(false);
        finishedCallback.current();
      } else {
        activeCallback.current(true);
      }
    }, controller.signal).catch((error) => {
      if (error?.name !== 'AbortError') notify('error', error?.message ?? 'Could not follow agent activity.');
    });
    return () => controller.abort();
  }, [selectedId, notify]);

  const selected = runs.find((run) => run.id === selectedId);
  const currentState = events[events.length - 1]?.state ?? selected?.state;
  const pending = currentState === 'WAITING_PERMISSION' && !permissionResolved
    ? [...events].reverse().find((event) => event.state === 'WAITING_PERMISSION' && event.pending_tool)
    : undefined;
  const active = !!selected && !TERMINAL.includes(selected.state);

  async function decide(approved: boolean, session = false) {
    if (!selected) return;
    setPermissionResolved(true);
    try {
      await resumeAgent(selected.id, approved, session);
      notify('info', approved ? 'Approved — work is continuing.' : 'Denied — the agent will adjust.');
    } catch (error: any) {
      setPermissionResolved(false);
      notify('error', error?.message ?? 'Could not record that decision.');
    }
  }

  if (!convId) {
    return <div className="activity-empty"><strong>No code session open</strong><span>Open or create a session to see its work.</span></div>;
  }

  if (!selected) {
    return (
      <div className="activity-empty">
        <span className="activity-orbit" aria-hidden="true" />
        <strong>Ready to work</strong>
        <span>Describe what you want in the composer. Planning, file changes, commands, and results will appear here.</span>
      </div>
    );
  }

  return (
    <div className="session-activity">
      <header className="work-pulse">
        <div className={`pulse-mark${active ? ' live' : ''}`} aria-hidden="true"><span /></div>
        <div>
          <span>{active ? 'Agent working' : selected.state === 'COMPLETED' ? 'Work completed' : selected.state === 'CANCELLED' ? 'Work stopped' : 'Work needs attention'}</span>
          <strong>{selected.task}</strong>
        </div>
        {active && <button onClick={() => stopAgent(selected.id).then(() => notify('info', 'Stopping agent…')).catch((error) => notify('error', error.message))}>Stop</button>}
      </header>
      <p className="panel-caption">{selected.iterations} step{selected.iterations === 1 ? '' : 's'} recorded · {active ? 'In progress' : 'Run ended'}</p>

      {pending?.pending_tool && (
        <section className="inline-permission" aria-label="Permission required">
          <strong>Approval needed</strong>
          <p>{pending.pending_tool.reason}</p>
          <code>{pending.pending_tool.tool}</code>
          <div>
            <button onClick={() => void decide(true)}>Allow once</button>
            <button onClick={() => void decide(true, true)}>Allow for session</button>
            <button className="quiet" onClick={() => void decide(false)}>Deny</button>
          </div>
        </section>
      )}

      <ToolTimeline events={events} />

      {runs.length > 1 && (
        <details className="session-runs">
          <summary>Earlier work in this session</summary>
          {runs.filter((run) => run.id !== selected.id).reverse().map((run) => (
            <button key={run.id} onClick={() => setSelectedId(run.id)}>
              <span>{run.state.toLowerCase()}</span>{run.task}
            </button>
          ))}
        </details>
      )}
    </div>
  );
}
