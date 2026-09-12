import { useState } from 'react';
import { artifactInfo } from '../services/api';
import { useEscape } from '../ui/primitives';

// Stage 28 attachment viewer (§74): zoom-ish preview + metadata + OCR text.
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
  void artifactInfo;
  useEscape(onClose);
  return (
    <div className="modal-backdrop" onClick={onClose} role="presentation">
      <div className="modal wide" role="dialog" aria-modal="true" aria-label={`Attachment ${att.filename}`} onClick={(e) => e.stopPropagation()}>
        <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
          <strong>{att.filename}</strong>
          <span style={{ flex: 1 }} />
          {isImage && <button onClick={() => setZoom((v) => !v)}>{zoom ? 'Fit' : 'Zoom'}</button>}
          <button onClick={onClose}>Close</button>
        </div>
        <div style={{ fontSize: 12, color: 'var(--text-secondary)', margin: '4px 0' }}>
          {att.mime} · {(att.size_bytes / 1024).toFixed(1)} KB · {att.kind}
          {att.status && att.status !== 'ready' ? ` · ${att.status}` : ''}
        </div>
        {isImage ? (
          <img
            src={`/api/conversations/${convId}/attachments/${att.id}/file`}
            alt={att.filename}
            style={zoom ? { width: '100%' } : { maxWidth: '100%', maxHeight: '60vh', objectFit: 'contain' }}
            onError={() => notify('error', 'Preview unavailable — the file is still on disk.')}
          />
        ) : (
          <pre className="diff-body">{att.text_excerpt || '(no extracted text)'}</pre>
        )}
      </div>
    </div>
  );
}
