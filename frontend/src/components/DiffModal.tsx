import { useEffect, useState } from 'react';
import { getWorkspaceDiff, getWorkspaceGit, type DiffInfo } from '../services/api';
import { Button, Dialog, Notice } from '../ui/primitives';
import { Icon } from '../ui/Icon';
import { DiffLines } from './ToolTimeline';

// Uncommitted changes in the project, with revert guidance.
export default function DiffModal({ wsId, onClose }: { wsId: string; onClose: () => void }) {
  const [d, setD] = useState<DiffInfo | null>(null);
  const [err, setErr] = useState('');
  const [notGit, setNotGit] = useState<string | null>(null);
  useEffect(() => {
    let active = true;
    // Ask Git first so a folder without a repository shows a clear state
    // instead of Git's usage text dressed up as a diff.
    getWorkspaceGit(wsId)
      .then((git) => {
        if (!active) return;
        if (!git.git) { setNotGit(git.detail ?? 'This project is not a Git repository.'); return; }
        return getWorkspaceDiff(wsId).then((diff) => { if (active) setD(diff); });
      })
      .catch((e) => { if (active) setErr(e.message); });
    return () => { active = false; };
  }, [wsId]);
  const clean = d && !d.diff.trim() && !d.stat.trim();
  return (
    <Dialog
      title="Changes"
      description="Uncommitted changes in this project’s working tree."
      icon="diff"
      size="xl"
      onClose={onClose}
      footer={<><span className="help" style={{ marginRight: 'auto' }}>Revert from your Git client, or ask the agent to revert specific files.</span><Button onClick={onClose}>Close</Button></>}
    >
      {err && <Notice tone="error" title="Couldn’t read changes">{err}</Notice>}
      {notGit && (
        <div className="empty-state" style={{ minHeight: 220 }}>
          <Icon name="branch" size={28} />
          <strong>Not a Git repository</strong>
          <p>Changes can only be listed for projects tracked by Git. Run “git init” in the project folder to start tracking.</p>
        </div>
      )}
      {!d && !err && !notGit && <p className="help">Reading changes…</p>}
      {clean && (
        <div className="empty-state" style={{ minHeight: 220 }}>
          <Icon name="checkCircle" size={28} />
          <strong>No uncommitted changes</strong>
          <p>The working tree matches the last commit.</p>
        </div>
      )}
      {d && !clean && (
        <>
          {d.stat && <pre className="diff-stat">{d.stat}</pre>}
          {d.diff && <DiffLines className="diff-view" value={`${d.diff.split('\n').slice(0, 400).join('\n')}${d.truncated ? '\n… truncated at 100K characters' : ''}`} />}
        </>
      )}
    </Dialog>
  );
}
