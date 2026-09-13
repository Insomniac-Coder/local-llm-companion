import { useEffect, useState } from 'react';
import { getWorkspaceGit } from '../services/api';
import { Button } from '../ui/primitives';

/** Current Git branch of a project, or '' when it has none. */
export function useWorkspaceBranch(wsId: string | undefined) {
  const [branch, setBranch] = useState('');
  useEffect(() => {
    setBranch('');
    if (!wsId) return;
    let active = true;
    getWorkspaceGit(wsId)
      .then((g) => { if (active && g.git && g.branch) setBranch(g.branch); })
      .catch(() => {});
    return () => { active = false; };
  }, [wsId]);
  return branch;
}

// Project commands live once, in the session header.
export default function ProjectActions({
  busy,
  onChanges,
  onCommand,
}: {
  busy: boolean;
  onChanges: () => void;
  onCommand: (command: string) => void;
}) {
  return (
    <div className="head-code-actions" role="group" aria-label="Project actions">
      <Button variant="ghost" size="sm" icon="diff" onClick={onChanges} data-tip="Review uncommitted changes">Changes</Button>
      <Button variant="ghost" size="sm" icon="wrench" disabled={busy} onClick={() => onCommand('/build')} data-tip="Run the project build">Build</Button>
      <Button variant="ghost" size="sm" icon="flask" disabled={busy} onClick={() => onCommand('/test')} data-tip="Run the project tests">Test</Button>
      <Button variant="ghost" size="sm" icon="play" disabled={busy} onClick={() => onCommand('/run ')} data-tip="Start a /run command">Run</Button>
    </div>
  );
}
