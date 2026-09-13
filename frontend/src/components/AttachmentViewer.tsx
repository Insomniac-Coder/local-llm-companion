import { useState } from 'react';
import { Button, Dialog } from '../ui/primitives';

// Attachment viewer: image preview with zoom, or the extracted text.
export default function AttachmentViewer({
  convId,
  att,
  onClose,
  notify,
}: {
  convId: string;
  att: { id: string; filename: string; mime: string; size_bytes: number; text_excerpt: string; kind: string; status?: string };
  onClose: () => void;
  notify: (k: 'info' | 'error', t: string) => void;
}) {
  const [zoom, setZoom] = useState(false);
  const isImage = att.kind === 'image';
  return (
    <Dialog
      title={att.filename}
      icon={isImage ? 'image' : 'fileText'}
      size="xl"
      onClose={onClose}
      footer={<>{isImage && <Button variant="ghost" icon={zoom ? 'panelLeft' : 'search'} onClick={() => setZoom((v) => !v)}>{zoom ? 'Fit to window' : 'Actual size'}</Button>}<span className="spacer" /><Button onClick={onClose}>Close</Button></>}
    >
      <p className="viewer-meta">{att.mime} · {(att.size_bytes / 1024).toFixed(1)} KB · {att.kind}{att.status && att.status !== 'ready' ? ` · ${att.status}` : ''}</p>
      {isImage ? (
        <img
          className={`viewer-image${zoom ? ' zoom' : ''}`}
          src={`/api/conversations/${convId}/attachments/${att.id}/file`}
          alt={att.filename}
          onError={() => notify('error', 'Preview unavailable — the file is still on disk.')}
        />
      ) : (
        <pre className="pre-block" style={{ maxHeight: '62dvh' }}>{att.text_excerpt || '(no extracted text)'}</pre>
      )}
    </Dialog>
  );
}
