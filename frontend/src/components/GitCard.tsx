import { useEffect, useState } from 'react';
import { executeTool, getWorkspaceGit, type GitInfo } from '../services/api';
import { Button, IconButton, Section } from '../ui/primitives';

// Git: read-only status plus a guarded commit box. Destructive Git
// (reset --hard, push --force) is refused server-side.
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
    <Section
      title="Git"
      icon="branch"
      meta={info.git ? info.branch || 'detached' : 'not a repository'}
      collapsible
      defaultOpen={false}
      actions={<IconButton icon="refresh" label="Refresh Git status" size="sm" tipSide="bottom-end" onClick={load} />}
    >
      {!info.git && <p className="help">{info.detail}</p>}
      {info.git && (
        <>
          <pre className="pre-block">{(info.status || '(clean)').slice(0, 2000)}</pre>
          <details className="tool-execution">
            <summary>Recent commits</summary>
            <pre className="pre-block">{(info.log || '(no history)').slice(0, 2000)}</pre>
          </details>
          <div className="inline-form">
            <input value={msg} onChange={(e) => setMsg(e.target.value)} placeholder="Commit message (one line)" aria-label="Commit message" />
            <Button onClick={commit} disabled={!msg.trim()} style={{ height: 34 }}>Commit staged</Button>
          </div>
          <p className="help">Commits staged changes only. Force-push and hard reset are refused — use your Git client for those.</p>
        </>
      )}
    </Section>
  );
}
