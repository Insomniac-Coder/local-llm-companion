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
import { Badge, Button } from '../ui/primitives';
import {
  listToolExecutions,
  type ContextInfo, type ToolDescriptor, type ToolExecution, type Workspace,
} from '../services/api';

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
  const [showMemory, setShowMemory] = useState(false);
  const [execs, setExecs] = useState<ToolExecution[]>([]);
  const ws = workspaces.find((workspace) => workspace.id === wsId);

  useEffect(() => {
    if (tab === 'tools' && convId) {
      listToolExecutions(convId, 20).then(setExecs).catch(() => setExecs([]));
    }
  }, [tab, convId]);

  if (tab === 'activity') {
    return <SessionActivity convId={convId} focusRun={focusRun} notify={notify} onFinished={onAgentFinished} onActiveChange={onAgentActiveChange} />;
  }

  if (tab === 'context') {
    return (
      <>
        {mode === 'code' && ws && (
          <div className="project-context"><span>Active project</span><strong>{ws.name}</strong><code>{ws.path}</code></div>
        )}
        <div className="card">
          <div className="panel-title-row">
            <strong>Context</strong>
            {ctx?.health && <Badge tone={ctx.health === 'healthy' ? 'ok' : ctx.health === 'moderate' ? 'info' : ctx.health === 'high' ? 'warn' : 'err'}>{ctx.health}</Badge>}
          </div>
          <ContextBar ctx={ctx} onCompact={onCompact} compacting={compacting} />
          {modelId && <RecommendCard modelId={modelId} notify={notify} />}
        </div>
        <InstructionsCard wsId={wsId} />
        {convId && (
          <>
            <Button variant="ghost" size="sm" onClick={() => setShowMemory((visible) => !visible)}>{showMemory ? '▾' : '▸'} Memories</Button>
            {showMemory && <MemoryPanel convId={convId} workspaceId={wsId} notify={notify} />}
          </>
        )}
        {!convId && <div className="activity-empty"><strong>No session open</strong><span>Open a session to inspect its context.</span></div>}
      </>
    );
  }

  if (tab === 'files') {
    if (!convId) return <div className="activity-empty"><strong>No files yet</strong><span>Open a session to see attachments and artifacts.</span></div>;
    return (
      <>
        <AttachmentsPanel convId={convId} notify={(kind, text) => notify(kind === 'error' ? 'error' : 'info', text)} />
        <ArtifactsPanel convId={convId} generating={false} />
        {mode === 'code' && wsId && <><RepoIndexCard wsId={wsId} notify={notify} /><KnowledgeCard wsId={wsId} notify={notify} /></>}
      </>
    );
  }

  return (
    <>
      <div className="card">
        <strong>Available tools</strong>
        <p className="panel-caption">{registry.length} local tools share the same approval boundary as this session.</p>
        {registry.slice(0, 8).map((tool) => <div className="tool-line" key={tool.name}><code>{tool.name}</code><span>{tool.risk}</span></div>)}
      </div>
      {mode === 'code' && wsId && <GitCard wsId={wsId} wsPath={wsPath} convId={convId} notify={notify} />}
      <div className="card">
        <strong>Recent executions</strong>
        {execs.length === 0 && <p className="panel-caption">Nothing has run in this session yet.</p>}
        {execs.map((execution) => (
          <details key={execution.id} className="tool-execution">
            <summary><code>{execution.tool.replace(/^chat:/, '')}</code><span>{execution.approved ? 'approved' : 'automatic'}</span></summary>
            <pre className="diff-stat">{execution.result.slice(0, 1500)}</pre>
          </details>
        ))}
      </div>
    </>
  );
}
