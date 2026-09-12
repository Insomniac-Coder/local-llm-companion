import { useEffect, useState } from 'react';
import { getRepoIndex, type RepoIndexInfo } from '../services/api';

// Stage 32 repo index card (§96): file/symbol counts, refresh, ranked lookup.
export default function RepoIndexCard({
  wsId,
  notify,
}: {
  wsId: string;
  notify: (k: 'info' | 'success' | 'warning' | 'error', t: string) => void;
}) {
  const [info, setInfo] = useState<RepoIndexInfo | null>(null);
  const [q, setQ] = useState('');
  const [open, setOpen] = useState(false);

  const load = (query = '', refresh = false) => {
    if (!wsId) return;
    getRepoIndex(wsId, query, refresh).then(setInfo).catch((e) => notify('error', e.message));
  };
  useEffect(() => {
    setInfo(null);
    setQ('');
    load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [wsId]);

  if (!wsId || !info) return null;
  return (
    <div className="card" style={{ margin: '0 16px' }}>
      <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
        <button className="ctx-toggle" onClick={() => setOpen((v) => !v)} aria-expanded={open}>
          {open ? '▾' : '▸'} Project index
        </button>
        <span style={{ fontSize: 12, color: 'var(--text-secondary)' }}>
          {info.files_total} files · {info.symbols_total} symbols{info.truncated ? ' · truncated' : ''}
        </span>
        <span style={{ flex: 1 }} />
        <button title="Rebuild the index" onClick={() => { load(q, true); notify('info', 'Index rebuilding…'); }}>
          Refresh
        </button>
      </div>
      {open && (
        <div style={{ marginTop: 6 }}>
          <div style={{ display: 'flex', gap: 6 }}>
            <input
              value={q}
              onChange={(e) => setQ(e.target.value)}
              onKeyDown={(e) => { if (e.key === 'Enter') load(q); }}
              placeholder="Find files/symbols…"
              style={{ flex: 1 }}
            />
            <button onClick={() => load(q)}>Find</button>
          </div>
          {info.matches && (
            <div style={{ fontSize: 13, marginTop: 6 }}>
              {info.matches.length === 0 && <div>No matches.</div>}
              {info.matches.slice(0, 15).map((m) => (
                <div key={m.path} style={{ marginTop: 2 }}>
                  <code>{m.path}</code>
                  {m.symbols.length > 0 && (
                    <span style={{ color: 'var(--text-secondary)' }}> — {m.symbols.slice(0, 3).join(' · ')}</span>
                  )}
                </div>
              ))}
            </div>
          )}
        </div>
      )}
    </div>
  );
}
