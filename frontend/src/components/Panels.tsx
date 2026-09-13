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
import { Button, Dialog, IconButton, Section } from '../ui/primitives';
import { Icon, type IconName } from '../ui/Icon';

const kb = (bytes: number) => bytes >= 1_048_576 ? `${(bytes / 1_048_576).toFixed(1)} MB` : `${(bytes / 1024).toFixed(1)} KB`;

function attachmentIcon(item: AttachmentInfo): IconName {
  if (item.status === 'unsupported' || item.status === 'partial') return 'alert';
  return item.kind === 'image' ? 'image' : 'fileText';
}

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
        if (!o.available) setOcr('No OCR engine is installed, so images use the vision model or a text fallback.');
      })
      .catch(() => {});
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [convId]);

  return (
    <Section title="Attachments" meta={items.length ? `${items.length}` : undefined} icon="paperclip">
      {items.length === 0 && <p className="help">Nothing attached. Drop a file on the composer or use the paperclip.</p>}
      {items.length > 0 && (
        <div className="list-rows">
          {items.map((a) => (
            <div key={a.id} className="list-row">
              <Icon name={attachmentIcon(a)} size={15} />
              <button type="button" className="link grow" onClick={() => setViewing(a)} title={`Open ${a.filename}`}>{a.filename}</button>
              <small>{kb(a.size_bytes)}{a.status && a.status !== 'ready' ? ` · ${a.status}` : ''}</small>
              <IconButton icon="x" label={`Remove ${a.filename}`} size="sm" tipSide="left" onClick={() => deleteAttachment(convId, a.id).then(load).catch((e) => notify('error', e.message))} />
            </div>
          ))}
        </div>
      )}
      {budget?.warnings.map((w, i) => <p key={i} className="help" style={{ color: 'var(--caution-ink)' }}>{w}</p>)}
      {ocr && items.some((item) => item.kind === 'image') && <p className="help">{ocr}</p>}
      {viewing && (
        <AttachmentViewer
          convId={convId}
          att={viewing}
          onClose={() => setViewing(null)}
          notify={(k, t) => notify(k === 'error' ? 'error' : 'info', t)}
        />
      )}
    </Section>
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
    <Section title="Generated files" meta={`${items.length}`} icon="box">
      {generating && <p className="help">Generating…</p>}
      <div className="list-rows">
        {items.map((a) => (
          <div key={a.id}>
            <div className="list-row">
              <Icon name="box" size={15} />
              <button type="button" className="link grow" onClick={() => setPreview({ name: a.filename, url: artifactUrl(a.id) })} title={`Preview ${a.filename}`}>{a.filename}</button>
              <small>{kb(a.size_bytes)}</small>
              <a className="icon-btn sm" href={artifactUrl(a.id)} download={a.filename} aria-label={`Save ${a.filename}`} data-tip="Save as" data-tip-side="left"><Icon name="download" size={14} /></a>
              <IconButton icon="folder" label="Copy file location" size="sm" tipSide="left" onClick={() => reveal(a.id)} />
            </div>
            {paths[a.id] && <p className="help mono">{paths[a.id]} — copied</p>}
          </div>
        ))}
      </div>
      {preview && <PreviewModal name={preview.name} url={preview.url} onClose={() => setPreview(null)} />}
    </Section>
  );
}

function PreviewModal({ name, url, onClose }: { name: string; url: string; onClose: () => void }) {
  return (
    <Dialog title={name} icon="box" size="xl" onClose={onClose} footer={<><a className="btn secondary" href={url} download={name}><Icon name="download" size={16} /><span className="btn-label">Save as</span></a><Button variant="ghost" onClick={onClose}>Close</Button></>}>
      <iframe className="viewer-frame" src={url} title={name} />
    </Dialog>
  );
}
