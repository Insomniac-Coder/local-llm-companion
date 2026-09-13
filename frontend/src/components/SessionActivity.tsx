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
import { Button, Lamp } from '../ui/primitives';
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
  const [liveText, setLiveText] = useState('');
  const [permissionResolved, setPermissionResolved] = useState(false);
  const [stopping, setStopping] = useState(false);
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
    setLiveText('');
    setPermissionResolved(false);
    setStopping(false);
    if (!selectedId) return;
    const controller = new AbortController();
    abort.current = controller;
    void streamAgentEvents(selectedId, (event) => {
      if (event.kind === 'thought_delta') {
        setLiveText((text) => text + event.message);
        return;
      }
      setLiveText('');
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
    return <div className="empty-state"><Icon name="code" size={28} /><strong>No code session open</strong><p>Open or create a session to see its work.</p></div>;
  }

  if (!selected) {
    return (
      <div className="empty-state">
        <Icon name="activity" size={28} />
        <strong>Nothing running yet</strong>
        <p>Describe a change in the composer. Planning, file changes, commands and results appear here as they happen.</p>
      </div>
    );
  }

  const summaryTone = active ? 'live' : selected.state === 'COMPLETED' ? 'done' : selected.state === 'CANCELLED' ? '' : 'failed';
  return (
    <div className="session-activity">
      <header className={`run-summary ${summaryTone}`}>
        <Lamp state={active ? (pending ? 'caution' : 'live') : selected.state === 'COMPLETED' ? 'ready' : selected.state === 'CANCELLED' ? 'off' : 'error'} pulse={active} />
        <div className="run-summary-copy">
          <span className="eyebrow">{active ? pending ? 'Waiting for you' : 'Agent working' : selected.state === 'COMPLETED' ? 'Work completed' : selected.state === 'CANCELLED' ? 'Work stopped' : 'Needs attention'}</span>
          <strong>{selected.task}</strong>
          <small className="readout">{selected.iterations} step{selected.iterations === 1 ? '' : 's'} recorded · {active ? 'in progress' : 'run ended'}</small>
        </div>
        {active && (
          <Button size="sm" variant="danger" icon="stop" loading={stopping} onClick={() => { setStopping(true); stopAgent(selected.id).then(() => notify('info', 'Stopping the agent…')).catch((error) => { setStopping(false); notify('error', error.message); }); }}>
            Stop
          </Button>
        )}
      </header>

      {pending?.pending_tool && (
        <section className="approval-card" aria-label="Permission required">
          <header><Icon name="shield" size={16} /> Approval needed</header>
          <p>{pending.pending_tool.reason}</p>
          <code>{pending.pending_tool.tool}</code>
          <div className="approval-actions">
            <Button size="sm" variant="secondary" icon="check" onClick={() => void decide(true)}>Allow once</Button>
            <Button size="sm" variant="ghost" onClick={() => void decide(true, true)}>Allow for session</Button>
            <Button size="sm" variant="danger" onClick={() => void decide(false)}>Deny</Button>
          </div>
        </section>
      )}

      <ToolTimeline events={events} />
      {active && liveText.trim() && (
        <div className="live-thought-block" aria-live="off">
          <span className="eyebrow">Writing now</span>
          <pre>{liveText}</pre>
        </div>
      )}

      {runs.length > 1 && (
        <details className="run-history">
          <summary><Icon name="history" size={14} /> Earlier work in this session</summary>
          {runs.filter((run) => run.id !== selected.id).reverse().map((run) => (
            <button type="button" key={run.id} onClick={() => setSelectedId(run.id)}>
              <span>{run.state.toLowerCase().replace(/_/g, ' ')}</span><span>{run.task}</span>
            </button>
          ))}
        </details>
      )}
    </div>
  );
}
