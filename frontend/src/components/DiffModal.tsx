import { useEffect, useState } from 'react';
import { getWorkspaceDiff, type DiffInfo } from '../services/api';
import { useEscape } from '../ui/primitives';

// Stage 24 polished diff presentation (§92) with revert guidance.
export default function DiffModal({ wsId, onClose }: { wsId: string; onClose: () => void }) {
  const [d, setD] = useState<DiffInfo | null>(null);
  const [err, setErr] = useState('');
  useEscape(onClose);
  useEffect(() => {
    getWorkspaceDiff(wsId).then(setD).catch((e) => setErr(e.message));
  }, [wsId]);
  return (
    <div className="modal-backdrop" onClick={onClose} role="presentation">
      <div className="modal" role="dialog" aria-modal="true" aria-label="Workspace diff" onClick={(e) => e.stopPropagation()}>
        <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
          <strong>Changes</strong>
          <span style={{ flex: 1 }} />
          <button onClick={onClose}>Close</button>
        </div>
        {err && <div className="approval">{err}</div>}
        {!d && !err && <div>Loading diff…</div>}
        {d && (
          <>
            <pre className="diff-stat">{d.stat || 'No changes.'}</pre>
            {d.diff && (
              <pre className="diff-body">
                {d.diff.split('\n').slice(0, 400).join('\n')}
                {d.truncated ? '\n…truncated at 100K chars' : ''}
              </pre>
            )}
            <div style={{ fontSize: 12, color: 'var(--text-secondary)', marginTop: 6 }}>
              Revert from your Git client or ask the agent to revert specific files.
            </div>
          </>
        )}
      </div>
    </div>
  );
}
