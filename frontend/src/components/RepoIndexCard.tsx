import { useEffect, useState } from 'react';
import { getRepoIndex, type RepoIndexInfo } from '../services/api';
import { Button, IconButton, Section } from '../ui/primitives';

// Project index: file and symbol counts, rebuild, ranked lookup.
export default function RepoIndexCard({
  wsId,
  notify,
}: {
  wsId: string;
  notify: (k: 'info' | 'success' | 'warning' | 'error', t: string) => void;
}) {
  const [info, setInfo] = useState<RepoIndexInfo | null>(null);
  const [q, setQ] = useState('');

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
    <Section
      title="Project index"
      icon="list"
      meta={`${info.files_total} files · ${info.symbols_total} symbols${info.truncated ? ' · truncated' : ''}`}
      collapsible
      defaultOpen={false}
      actions={<IconButton icon="refresh" label="Rebuild the index" size="sm" tipSide="bottom-end" onClick={() => { load(q, true); notify('info', 'Rebuilding the project index…'); }} />}
    >
      <div className="inline-form">
        <input value={q} onChange={(e) => setQ(e.target.value)} onKeyDown={(e) => { if (e.key === 'Enter') load(q); }} placeholder="Find files or symbols" aria-label="Find files or symbols" />
        <Button onClick={() => load(q)} style={{ height: 34 }}>Find</Button>
      </div>
      {info.matches && (
        <div className="list-rows">
          {info.matches.length === 0 && <p className="help">No matches.</p>}
          {info.matches.slice(0, 15).map((m) => (
            <div key={m.path} className="list-row" style={{ alignItems: 'flex-start', flexDirection: 'column', gap: 2 }}>
              <code>{m.path}</code>
              {m.symbols.length > 0 && <small>{m.symbols.slice(0, 3).join(' · ')}</small>}
            </div>
          ))}
        </div>
      )}
    </Section>
  );
}
