import { useEffect, useState } from 'react';
import { Chip } from '../ui/primitives';
import { deleteAttachment, listAttachments, type AttachmentInfo } from '../services/api';

// Design guide §7: attachment chips show type, name, size, processing state.
export default function AttachChips({
  convId,
  tick,
  notify,
}: {
  convId: string | null;
  tick: number;
  notify: (k: 'info' | 'error', t: string) => void;
}) {
  const [items, setItems] = useState<AttachmentInfo[]>([]);
  useEffect(() => {
    if (!convId) {
      setItems([]);
      return;
    }
    listAttachments(convId).then(setItems).catch(() => setItems([]));
  }, [convId, tick]);
  if (!convId || items.length === 0) return null;
  const icon = (a: AttachmentInfo) =>
    a.kind === 'image' ? '🖼' : a.status === 'unsupported' ? '✕' : a.status === 'partial' ? '⚠' : '📄';
  return (
    <div style={{ display: 'flex', gap: 6, flexWrap: 'wrap', padding: '8px 16px 0' }} aria-label="Attachments">
      {items.map((a) => (
        <span key={a.id} style={{ display: 'inline-flex', alignItems: 'center', gap: 4 }}>
          <Chip title={`${a.mime} · ${(a.size_bytes / 1024).toFixed(1)} KB · ${a.status ?? 'ready'}`}>
            {icon(a)} {a.filename} · {(a.size_bytes / 1024).toFixed(1)} KB
          </Chip>
          <button
            className="ctx-toggle"
            onClick={() => deleteAttachment(convId, a.id).then(() => setItems((p) => p.filter((x) => x.id !== a.id))).catch((e) => notify('error', e.message))}
            aria-label={`Remove ${a.filename}`}
            title="Remove"
          >
            ×
          </button>
        </span>
      ))}
    </div>
  );
}
