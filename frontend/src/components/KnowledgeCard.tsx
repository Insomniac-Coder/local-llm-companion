import { useEffect, useState } from 'react';
import { clearKnowledge, ingestKnowledge, listKnowledge, type KnowledgeInfo } from '../services/api';

// Stage 35 local knowledge card (§39): ingest folders/files, keyword recall.
export default function KnowledgeCard({
  wsId,
  notify,
}: {
  wsId: string;
  notify: (k: 'info' | 'success' | 'warning' | 'error', t: string) => void;
}) {
  const [info, setInfo] = useState<KnowledgeInfo | null>(null);
  const [path, setPath] = useState('');
  const [open, setOpen] = useState(false);

  const load = () => {
    if (!wsId) return;
    listKnowledge(wsId).then(setInfo).catch(() => setInfo(null));
  };
  useEffect(() => {
    setInfo(null);
    load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [wsId]);

  if (!wsId) return null;
  return (
    <div className="card" style={{ margin: '0 16px' }}>
      <button className="ctx-toggle" onClick={() => setOpen((v) => !v)} aria-expanded={open}>
        {open ? '▾' : '▸'} Knowledge{info ? ` (${info.total_chunks} chunks)` : ''}
      </button>
      {open && (
        <div style={{ marginTop: 6 }}>
          <div style={{ fontSize: 12, color: 'var(--text-secondary)' }}>
            Local-only docs the assistant searches by keywords. No embeddings yet — plain term overlap.
          </div>
          {(info?.paths ?? []).map((p) => (
            <div key={p.path} style={{ display: 'flex', gap: 8, fontSize: 13, marginTop: 4, alignItems: 'center' }}>
              <span style={{ flex: 1 }}>📄 {p.path} · {p.chunks} chunks</span>
              <button
                onClick={() => clearKnowledge(wsId, p.path).then(() => { load(); notify('success', `Removed ${p.path}.`); }).catch((e) => notify('error', e.message))}
                title="Remove from knowledge"
              >
                ×
              </button>
            </div>
          ))}
          {(info?.paths ?? []).length === 0 && <div style={{ fontSize: 13 }}>Nothing indexed yet.</div>}
          <div style={{ display: 'flex', gap: 6, marginTop: 8 }}>
            <input value={path} onChange={(e) => setPath(e.target.value)} placeholder="docs/ or notes.md (workspace-relative)" style={{ flex: 1 }} />
            <button
              disabled={!path.trim()}
              onClick={() => ingestKnowledge(wsId, path.trim()).then((r: any) => {
                setPath('');
                load();
                notify('success', `Indexed ${r.files_indexed} file(s), ${r.chunks_added} chunks.`);
              }).catch((e) => notify('error', e.message))}
            >
              Index
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
