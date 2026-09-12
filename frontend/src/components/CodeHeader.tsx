import { useEffect, useState } from 'react';
import { getWorkspaceGit, type Workspace } from '../services/api';

// Project context stays visible as a breadcrumb. Commands live in the single
// composer toolbar so the interface never presents duplicate actions.
export default function CodeHeader({
  wsId,
  workspaces,
  busy,
  onChanges,
  onCommand,
}: {
  wsId: string;
  workspaces: Workspace[];
  busy: boolean;
  onChanges: () => void;
  onCommand: (command: string) => void;
}) {
  const [branch, setBranch] = useState('');
  const ws = workspaces.find((w) => w.id === wsId);
  useEffect(() => {
    setBranch('');
    if (!wsId) return;
    getWorkspaceGit(wsId)
      .then((g) => {
        if (g.git && g.branch) setBranch(g.branch);
      })
      .catch(() => {});
  }, [wsId]);
  if (!ws) return null;
  return (
    <div className="codeheader" aria-label="Project header">
      <span className="project-mark" aria-hidden="true"><i /></span>
      <span className="proj">{ws.name}</span>
      <span className="crumb" aria-hidden="true">/</span>
      <span className="sub project-path" title={ws.path}>{ws.path}</span>
      <span className="codeheader-spacer" />
      {branch && <span className="sub branch" title="Git branch"><span aria-hidden="true">⑂</span> {branch}</span>}
      <div className="project-actions-bar" aria-label="Project actions">
        <button onClick={onChanges}>Changes</button>
        <button disabled={busy} onClick={() => onCommand('/build')}>Build</button>
        <button disabled={busy} onClick={() => onCommand('/test')}>Test</button>
        <button disabled={busy} onClick={() => onCommand('/run ')}>Run…</button>
      </div>
    </div>
  );
}
