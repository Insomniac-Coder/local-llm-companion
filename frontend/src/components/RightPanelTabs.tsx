import { useEffect, useState } from 'react';
import { ArtifactsPanel, AttachmentsPanel } from './Panels';
import ContextBar from './ContextBar';
import InstructionsCard from './InstructionsCard';
import MemoryPanel from './MemoryPanel';
import RepoIndexCard from './RepoIndexCard';
import KnowledgeCard from './KnowledgeCard';
import GitCard from './GitCard';
import RecommendCard from './RecommendCard';
import SessionActivity from './SessionActivity';
import { Badge, IconButton, Section } from '../ui/primitives';
import { Icon } from '../ui/Icon';
import {
  listToolExecutions,
  type ContextInfo, type ToolDescriptor, type ToolExecution, type Workspace,
} from '../services/api';

const riskTone = (risk: string) => risk.toLowerCase().startsWith('danger') ? 'err' : risk.toLowerCase().startsWith('moderate') ? 'warn' : 'neutral';

export default function RightPanelTabs({
  tab,
  convId,
  wsId,
  wsPath,
  mode,
  modelId,
  ctx,
  compacting,
  onCompact,
  workspaces,
  registry,
  notify,
  focusRun,
  onAgentFinished,
  onAgentActiveChange,
}: {
  tab: string;
  convId: string | null;
  wsId: string;
  wsPath: string;
  mode: 'chat' | 'code';
  modelId: string;
  ctx: ContextInfo | null;
  compacting: boolean;
  onCompact: () => void;
  workspaces: Workspace[];
  registry: ToolDescriptor[];
  notify: (k: 'info' | 'success' | 'warning' | 'error', t: string) => void;
  focusRun?: string | null;
  onAgentFinished: () => void;
  onAgentActiveChange: (active: boolean) => void;
}) {
  const [execs, setExecs] = useState<ToolExecution[]>([]);
  const [execsTick, setExecsTick] = useState(0);
  const ws = workspaces.find((workspace) => workspace.id === wsId);

  useEffect(() => {
    if (tab === 'tools' && convId) {
      listToolExecutions(convId, 20).then(setExecs).catch(() => setExecs([]));
    }
  }, [tab, convId, execsTick]);

  if (tab === 'activity' && mode === 'code') {
    return <SessionActivity convId={convId} focusRun={focusRun} notify={notify} onFinished={onAgentFinished} onActiveChange={onAgentActiveChange} />;
  }

  if (tab === 'files') {
    if (!convId) return <div className="empty-state"><Icon name="file" size={28} /><strong>No files yet</strong><p>Open a session to see its attachments and generated files.</p></div>;
    return (
      <>
        <AttachmentsPanel convId={convId} notify={(kind, text) => notify(kind === 'error' ? 'error' : 'info', text)} />
        <ArtifactsPanel convId={convId} generating={false} />
        {mode === 'code' && wsId && <><RepoIndexCard wsId={wsId} notify={notify} /><KnowledgeCard wsId={wsId} notify={notify} /></>}
      </>
    );
  }

  if (tab === 'tools' && mode === 'code') {
    return (
      <>
        <Section title="Available tools" meta={`${registry.length}`} icon="wrench">
          <p className="help">Every tool passes the same approval policy as this session.</p>
          <div className="list-rows">
            {registry.map((tool) => (
              <div className="list-row" key={tool.name} title={tool.description}>
                <code className="grow">{tool.name}</code>
                <Badge tone={riskTone(tool.risk)}>{tool.risk.toLowerCase().startsWith('safe') ? 'Read-only' : tool.risk}</Badge>
              </div>
            ))}
          </div>
        </Section>
        {wsId && <GitCard wsId={wsId} wsPath={wsPath} convId={convId} notify={notify} />}
        <Section title="Recent executions" meta={execs.length ? `${execs.length}` : undefined} icon="history" actions={<IconButton icon="refresh" label="Refresh executions" size="sm" tipSide="bottom-end" onClick={() => setExecsTick((value) => value + 1)} />}>
          {execs.length === 0 && <p className="help">Nothing has run in this session yet.</p>}
          <div>
            {execs.map((execution) => (
              <details key={execution.id} className="tool-execution">
                <summary><Icon name="chevronRight" size={13} /><code>{execution.tool.replace(/^chat:/, '')}</code><small className="readout">{execution.approved ? 'approved' : 'automatic'}</small></summary>
                <pre className="pre-block">{execution.result.slice(0, 1500)}</pre>
              </details>
            ))}
          </div>
        </Section>
      </>
    );
  }

  return (
    <>
      {mode === 'code' && ws && (
        <div className="insp-project"><Icon name="folder" size={18} /><div><strong>{ws.name}</strong><code>{ws.path}</code></div></div>
      )}
      <Section title="Context" icon="gauge" actions={ctx?.health ? <Badge tone={ctx.health === 'healthy' ? 'ok' : ctx.health === 'moderate' ? 'info' : ctx.health === 'high' ? 'warn' : 'err'}>{ctx.health}</Badge> : undefined}>
        {convId ? <ContextBar ctx={ctx} onCompact={onCompact} compacting={compacting} /> : <p className="help">Open a session to inspect its context.</p>}
      </Section>
      {modelId && <RecommendCard modelId={modelId} notify={notify} />}
      <InstructionsCard wsId={wsId} />
      {convId && <MemoryPanel convId={convId} workspaceId={wsId} notify={notify} />}
    </>
  );
}
