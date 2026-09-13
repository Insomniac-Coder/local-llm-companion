import { useEffect, useState } from 'react';
import { clearKnowledge, ingestKnowledge, listKnowledge, type KnowledgeInfo } from '../services/api';
import { Button, IconButton, Section } from '../ui/primitives';
import { Icon } from '../ui/Icon';

// Local knowledge: ingest folders or files for keyword recall.
export default function KnowledgeCard({
  wsId,
  notify,
}: {
  wsId: string;
  notify: (k: 'info' | 'success' | 'warning' | 'error', t: string) => void;
}) {
  const [info, setInfo] = useState<KnowledgeInfo | null>(null);
  const [path, setPath] = useState('');

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
    <Section title="Knowledge" icon="book" meta={info ? `${info.total_chunks} chunks` : undefined} collapsible defaultOpen={false}>
      <p className="help">Local documents the assistant searches by keyword. Nothing leaves this PC; matching is plain term overlap, not embeddings.</p>
      {(info?.paths ?? []).length > 0 && (
        <div className="list-rows">
          {(info?.paths ?? []).map((p) => (
            <div key={p.path} className="list-row">
              <Icon name="fileText" size={15} />
              <span className="grow" title={p.path}>{p.path}</span>
              <small>{p.chunks} chunks</small>
              <IconButton icon="x" label={`Remove ${p.path} from knowledge`} size="sm" tipSide="left" onClick={() => clearKnowledge(wsId, p.path).then(() => { load(); notify('success', `Removed ${p.path}.`); }).catch((e) => notify('error', e.message))} />
            </div>
          ))}
        </div>
      )}
      {(info?.paths ?? []).length === 0 && <p className="help">Nothing indexed yet.</p>}
      <div className="inline-form">
        <input value={path} onChange={(e) => setPath(e.target.value)} placeholder="docs/ or notes.md" aria-label="Project-relative path to index" />
        <Button
          style={{ height: 34 }}
          disabled={!path.trim()}
          onClick={() => ingestKnowledge(wsId, path.trim()).then((r: any) => {
            setPath('');
            load();
            notify('success', `Indexed ${r.files_indexed} file(s), ${r.chunks_added} chunks.`);
          }).catch((e) => notify('error', e.message))}
        >
          Index
        </Button>
      </div>
    </Section>
  );
}
