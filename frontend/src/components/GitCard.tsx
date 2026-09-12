import { useEffect, useState } from 'react';
import { executeTool, getWorkspaceGit, type GitInfo } from '../services/api';

// Stage 36 git card (§62): read-only status plus a guarded commit box.
// Destructive git (reset --hard, push --force) is refused server-side.
export default function GitCard({
  wsId,
  wsPath,
  convId,
  notify,
}: {
  wsId: string;
  wsPath: string;
  convId: string | null;
  notify: (k: 'info' | 'success' | 'warning' | 'error', t: string) => void;
}) {
  const [info, setInfo] = useState<GitInfo | null>(null);
  const [open, setOpen] = useState(false);
  const [msg, setMsg] = useState('');

  const load = () => {
    if (!wsId) return;
    getWorkspaceGit(wsId).then(setInfo).catch((e) => notify('error', e.message));
  };
  useEffect(() => {
    setInfo(null);
    load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [wsId]);

  if (!wsId || !info) return null;
  const commit = () => {
    if (!msg.trim()) {
      notify('warning', 'Write a commit message first.');
      return;
    }
    if (!confirm(`Commit staged changes with message:\n\n${msg.trim()}`)) return;
    executeTool(wsPath, 'git_commit', { message: msg.trim() }, true, convId ?? '')
      .then((r: any) => {
        setMsg('');
        load();
        notify(r.ok ? 'success' : 'warning', r.ok ? 'Committed.' : `Commit exited non-zero: ${String(r.output ?? '').slice(0, 200)}`);
      })
      .catch((e) => notify('error', e.message));
  };

  return (
    <div className="card" style={{ margin: '0 16px' }}>
      <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
        <button className="ctx-toggle" onClick={() => setOpen((v) => !v)} aria-expanded={open}>
          {open ? '▾' : '▸'} Git{info.git && info.branch ? ` (${info.branch})` : ''}
        </button>
        <span style={{ flex: 1 }} />
        <button title="Refresh git state" onClick={load}>Refresh</button>
      </div>
      {open && !info.git && <div style={{ fontSize: 13, marginTop: 4 }}>{info.detail}</div>}
      {open && info.git && (
        <div style={{ marginTop: 6 }}>
          <pre className="diff-stat">{(info.status || '(clean)').slice(0, 2000)}</pre>
          <details style={{ marginTop: 4 }}>
            <summary style={{ fontSize: 13 }}>Recent commits</summary>
            <pre className="diff-stat">{(info.log || '(no history)').slice(0, 2000)}</pre>
          </details>
          <div style={{ display: 'flex', gap: 6, marginTop: 8 }}>
            <input value={msg} onChange={(e) => setMsg(e.target.value)} placeholder="Commit message (single line)" style={{ flex: 1 }} />
            <button onClick={commit}>Commit staged</button>
          </div>
          <div style={{ fontSize: 12, color: 'var(--text-secondary)', marginTop: 4 }}>
            Commits staged changes only. Force-push and hard reset are refused — do those in your Git client.
          </div>
        </div>
      )}
    </div>
  );
}
