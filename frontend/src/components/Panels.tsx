import { useEffect, useState } from 'react';
import {
  artifactInfo,
  artifactUrl,
  deleteAttachment,
  getAttachmentBudget,
  listArtifacts,
  listAttachments,
  ocrStatus,
  type ArtifactInfo,
  type AttachmentBudget,
  type AttachmentInfo,
} from '../services/api';
import AttachmentViewer from './AttachmentViewer';
import { useEscape } from '../ui/primitives';

const STATUS_ICON: Record<string, string> = { ready: '✓', partial: '⚠', processing: '●', unsupported: '✕' };

export function AttachmentsPanel({
  convId,
  notify,
}: {
  convId: string;
  notify: (kind: 'info' | 'success' | 'warning' | 'error', text: string) => void;
}) {
  const [items, setItems] = useState<AttachmentInfo[]>([]);
  const [budget, setBudget] = useState<AttachmentBudget | null>(null);
  const [ocr, setOcr] = useState('');
  const [viewing, setViewing] = useState<AttachmentInfo | null>(null);

  const load = () => {
    listAttachments(convId).then(setItems).catch(() => setItems([]));
    getAttachmentBudget(convId).then(setBudget).catch(() => setBudget(null));
  };

  useEffect(() => {
    load();
    ocrStatus()
      .then((o) => {
        if (!o.available) setOcr('OCR engine not installed — images use the vision model or a text fallback.');
      })
      .catch(() => {});
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [convId]);

  if (items.length === 0) return null;

  return (
    <div className="card" style={{ margin: '0 16px' }}>
      <strong>Attachments ({items.length})</strong>
      {items.map((a) => (
        <div key={a.id} style={{ display: 'flex', gap: 8, fontSize: 13, marginTop: 4, alignItems: 'center' }}>
          <span title={a.status ?? 'ready'}>{a.kind === 'image' ? '🖼' : '📄'}</span>
          <button className="ctx-toggle" style={{ flex: 1, textAlign: 'left' }} onClick={() => setViewing(a)} title="Open viewer">
            {a.filename} · {(a.size_bytes / 1024).toFixed(1)} KB
          </button>
          <span style={{ color: 'var(--text-muted)', fontSize: 12 }} title={`extraction: ${a.status ?? 'ready'}`}>
            {STATUS_ICON[a.status ?? 'ready'] ?? ''}
          </span>
          <button
            onClick={() => deleteAttachment(convId, a.id).then(load).catch((e) => notify('error', e.message))}
            title="Remove attachment"
          >
            ×
          </button>
        </div>
      ))}
      {budget?.warnings.map((w, i) => (
        <div key={i} className="approval" style={{ marginTop: 4, fontSize: 12 }}>
          {w}
        </div>
      ))}
      {ocr && <div style={{ fontSize: 12, color: 'var(--text-muted)', marginTop: 4 }}>{ocr}</div>}
      {viewing && (
        <AttachmentViewer
          convId={convId}
          att={viewing}
          onClose={() => setViewing(null)}
          notify={(k, t) => notify(k === 'error' ? 'error' : 'info', t)}
        />
      )}
    </div>
  );
}

export function ArtifactsPanel({ convId, generating }: { convId: string; generating: boolean }) {
  const [items, setItems] = useState<ArtifactInfo[]>([]);
  const [paths, setPaths] = useState<Record<string, string>>({});
  const [preview, setPreview] = useState<{ name: string; url: string } | null>(null);

  useEffect(() => {
    listArtifacts(convId).then(setItems).catch(() => setItems([]));
  }, [convId]);

  useEffect(() => {
    let stop = false;
    if (!generating) return;
    const t = setInterval(() => {
      listArtifacts(convId).then((v) => {
        if (!stop) setItems(v);
      }).catch(() => {});
    }, 3000);
    return () => {
      stop = true;
      clearInterval(t);
    };
  }, [convId, generating]);

  const reveal = (id: string) => {
    artifactInfo(id)
      .then((d) => {
        setPaths((p) => ({ ...p, [id]: d.path }));
        void navigator.clipboard?.writeText(d.path).catch(() => {});
      })
      .catch(() => {});
  };

  if (items.length === 0 && !generating) return null;

  return (
    <div className="card" style={{ margin: '0 16px' }}>
      <strong>
        Generated {items.length} artifact{items.length === 1 ? '' : 's'}
      </strong>
      {generating && <div style={{ fontSize: 12 }}>● Generating…</div>}
      {items.map((a) => (
        <div key={a.id} style={{ display: 'flex', gap: 8, fontSize: 13, marginTop: 4, alignItems: 'center', flexWrap: 'wrap' }}>
          <span>📦</span>
          <span style={{ flex: 1 }}>{a.filename} · {(a.size_bytes / 1024).toFixed(1)} KB</span>
          <button onClick={() => setPreview({ name: a.filename, url: artifactUrl(a.id) })}>Open</button>
          <a href={artifactUrl(a.id)} download={a.filename}>
            <button>Save As</button>
          </a>
          <button onClick={() => reveal(a.id)} title="Show file location (copies path)">
            Reveal
          </button>
        </div>
      ))}
      {Object.entries(paths).map(([id, p]) => (
        <div key={id} style={{ fontSize: 12, color: 'var(--text-secondary)', marginTop: 2 }}>
          {p} (copied)
        </div>
      ))}
      {preview && (
        <PreviewModal name={preview.name} url={preview.url} onClose={() => setPreview(null)} />
      )}
    </div>
  );
}

function PreviewModal({ name, url, onClose }: { name: string; url: string; onClose: () => void }) {
  useEscape(onClose);
  return (
    <div className="modal-backdrop" onClick={onClose} role="presentation">
      <div className="modal wide" role="dialog" aria-modal="true" aria-label={name} onClick={(e) => e.stopPropagation()}>
        <div style={{ display: 'flex', gap: 8 }}>
          <strong>{name}</strong>
          <span style={{ flex: 1 }} />
          <button onClick={onClose}>Close</button>
        </div>
        <iframe src={url} title={name} style={{ width: '100%', height: '60vh', marginTop: 8 }} />
      </div>
    </div>
  );
}
