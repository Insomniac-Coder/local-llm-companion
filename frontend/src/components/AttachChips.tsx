import { useEffect, useState } from 'react';
import { deleteAttachment, listAttachments, type AttachmentInfo } from '../services/api';
import { IconButton } from '../ui/primitives';
import { Icon } from '../ui/Icon';

// Attachment chips show type, name, size and processing state.
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
  return (
    <div className="attach-chips" aria-label="Attachments">
      {items.map((a) => (
        <span key={a.id} className={`attach-chip ${a.status ?? 'ready'}`} title={`${a.mime} · ${(a.size_bytes / 1024).toFixed(1)} KB · ${a.status ?? 'ready'}`}>
          <Icon name={a.status === 'unsupported' || a.status === 'partial' ? 'alert' : a.kind === 'image' ? 'image' : 'fileText'} size={14} />
          <span>{a.filename}</span>
          <small>{(a.size_bytes / 1024).toFixed(0)} KB</small>
          <IconButton
            icon="x"
            size="sm"
            tip={false}
            label={`Remove ${a.filename}`}
            onClick={() => deleteAttachment(convId, a.id).then(() => setItems((p) => p.filter((x) => x.id !== a.id))).catch((e) => notify('error', e.message))}
          />
        </span>
      ))}
    </div>
  );
}
