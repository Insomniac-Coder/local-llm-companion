import { useState } from 'react';
import { downloadAction, scanModels, startDownload, type DownloadInfo, type ModelMeta } from '../services/api';
import ModelLibraryItem from './ModelLibraryItem';
import SetupWizard from './SetupWizard';
import { Button, IconButton, Section } from '../ui/primitives';
import { Icon } from '../ui/Icon';

type Notify = (kind: 'info' | 'success' | 'warning' | 'error', text: string) => void;

const mb = (bytes: number) => (bytes / 1048576).toFixed(1);

export default function ModelsPage({
  models,
  loadingModel,
  downloads,
  notify,
  onLoad,
  onDelete,
  refreshModels,
  refreshDownloads,
}: {
  models: ModelMeta[];
  loadingModel: boolean;
  downloads: DownloadInfo[];
  notify: Notify;
  onLoad: (id: string) => void;
  onDelete: (model: ModelMeta) => void;
  refreshModels: () => unknown;
  refreshDownloads: () => unknown;
}) {
  const [downloadOpen, setDownloadOpen] = useState(false);
  const [dlId, setDlId] = useState('');
  const [dlUrl, setDlUrl] = useState('');
  const [dlSha, setDlSha] = useState('');
  const [scanning, setScanning] = useState(false);
  const loadedCount = models.filter((model) => model.loaded).length;

  const scan = () => {
    setScanning(true);
    scanModels()
      .then((r) => {
        notify(r.warnings.length ? 'warning' : 'success', `Scan found ${r.registered} model${r.registered === 1 ? '' : 's'}${r.warnings.length ? ` with ${r.warnings.length} warning${r.warnings.length === 1 ? '' : 's'}: ${r.warnings.join(' ')}` : '.'}`);
        return refreshModels();
      })
      .catch((e) => notify('error', e.message))
      .finally(() => setScanning(false));
  };

  const download = () => startDownload(dlId.trim(), dlUrl.trim(), dlSha.trim() || undefined)
    .then(() => { notify('success', `Download '${dlId.trim()}' started.`); setDlId(''); setDlUrl(''); setDlSha(''); setDownloadOpen(false); refreshDownloads(); })
    .catch((e) => notify('error', e.message));

  const act = (id: string, action: 'pause' | 'resume' | 'cancel') => downloadAction(id, action).then(refreshDownloads).catch((e) => notify('error', e.message));

  return (
    <div className="page">
      <div className="page-inner">
        <SetupWizard notify={(kind, text) => notify(kind, text)} />

        <div className="page-toolbar">
          <span className="readout page-count">{models.length} installed{loadedCount ? ` · ${loadedCount} loaded` : ''}</span>
          <span className="spacer" />
          <Button icon="refresh" loading={scanning} onClick={scan} data-tip="Find GGUF files in the models folder and its subfolders">Scan models folder</Button>
          <Button icon="download" aria-expanded={downloadOpen} onClick={() => setDownloadOpen((open) => !open)}>Download a model</Button>
        </div>

        {downloadOpen && (
          <form className="panel download-panel" onSubmit={(event) => { event.preventDefault(); if (dlId.trim() && dlUrl.trim()) void download(); }}>
            <div className="panel-head">
              <div><h2>Download a model</h2><p>Paste a direct link to a .gguf file, such as a Hugging Face “resolve” URL. Interrupted downloads resume where they stopped.</p></div>
              <IconButton icon="x" label="Close download form" tipSide="bottom-end" onClick={() => setDownloadOpen(false)} />
            </div>
            <div className="download-grid">
              <label className="field"><span>Name</span><input value={dlId} onChange={(e) => setDlId(e.target.value)} placeholder="my-model" autoFocus /></label>
              <label className="field wide"><span>File link</span><input value={dlUrl} onChange={(e) => setDlUrl(e.target.value)} placeholder="https://huggingface.co/…/model.gguf" /></label>
              <label className="field wide"><span>SHA-256 <em className="muted" style={{ fontStyle: 'normal' }}>(optional)</em></span><input value={dlSha} onChange={(e) => setDlSha(e.target.value)} placeholder="Verifies the file after download" /></label>
            </div>
            <div className="panel-foot"><Button variant="ghost" onClick={() => setDownloadOpen(false)}>Cancel</Button><Button type="submit" icon="download" disabled={!dlId.trim() || !dlUrl.trim()}>Start download</Button></div>
          </form>
        )}

        {downloads.length > 0 && (
          <div className="panel">
            <Section title="Downloads" meta={`${downloads.length}`} icon="download">
              <div className="list-rows">
                {downloads.map((d) => (
                  <div key={d.id} className="download-row">
                    <div className="download-row-head">
                      <strong>{d.id}</strong>
                      <span className="readout">{d.status} · {mb(d.downloaded_bytes)}{d.total_bytes ? ` / ${mb(d.total_bytes)}` : ''} MB</span>
                      <span className="spacer" />
                      {d.status === 'downloading' && <IconButton icon="pause" label="Pause download" size="sm" tipSide="left" onClick={() => void act(d.id, 'pause')} />}
                      {(d.status === 'paused' || d.status === 'failed' || d.status === 'cancelled') && <IconButton icon="play" label="Resume download" size="sm" tipSide="left" onClick={() => void act(d.id, 'resume')} />}
                      {(d.status === 'downloading' || d.status === 'paused') && <IconButton icon="x" label="Cancel download" size="sm" tone="danger" tipSide="left" onClick={() => void act(d.id, 'cancel')} />}
                    </div>
                    <progress value={d.downloaded_bytes} max={d.total_bytes ?? (d.downloaded_bytes || 1)} />
                    {d.error && <p className="help" style={{ color: 'var(--error-ink)' }}>{d.error}</p>}
                  </div>
                ))}
              </div>
            </Section>
          </div>
        )}

        {models.length === 0 ? (
          <div className="panel empty-state" style={{ minHeight: 260 }}>
            <Icon name="layers" size={30} />
            <strong>No models yet</strong>
            <p>Put a .gguf file in the models folder and scan, or download one with a direct link.</p>
          </div>
        ) : (
          <div className="model-list">
            {models.map((m) => (
              <ModelLibraryItem key={m.id} model={m} loadingModel={loadingModel} notify={notify}
                onToolingChecked={() => void refreshModels()}
                onLoad={() => onLoad(m.id)}
                onDelete={() => onDelete(m)}
              />
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
