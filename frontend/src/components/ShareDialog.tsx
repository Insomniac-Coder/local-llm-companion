import { useState } from 'react';
import { useEscape } from '../ui/primitives';

// Stage 24 share picker (§§98–99): explicit include checkboxes, no leakage.
export interface ShareOptions {
  summary: boolean;
  messages: boolean;
  attachments: boolean;
  memory: boolean;
  turns: number;
  note: string;
}

export default function ShareDialog({
  targets,
  onShare,
  onClose,
}: {
  targets: { id: string; label: string }[];
  onShare: (targetId: string, opts: ShareOptions) => void;
  onClose: () => void;
}) {
  const [target, setTarget] = useState('');
  const [opts, setOpts] = useState<ShareOptions>({ summary: true, messages: true, attachments: false, memory: false, turns: 6, note: '' });
  const flip = (k: keyof ShareOptions) => setOpts((o) => ({ ...o, [k]: !o[k] }));
  useEscape(onClose);
  return (
    <div className="modal-backdrop" onClick={onClose} role="presentation">
      <div className="modal" role="dialog" aria-modal="true" aria-label="Share context" onClick={(e) => e.stopPropagation()}>
        <strong>Share context</strong>
        <div style={{ marginTop: 8, display: 'flex', flexDirection: 'column', gap: 6 }}>
          <label>
            Destination
            <select value={target} onChange={(e) => setTarget(e.target.value)} style={{ width: '100%', marginTop: 2 }}>
              <option value="">— destination session —</option>
              {targets.map((t) => (
                <option key={t.id} value={t.id}>{t.label}</option>
              ))}
            </select>
          </label>
          <label style={{ fontSize: 13 }}>
            <input type="checkbox" checked={opts.summary} onChange={() => flip('summary')} /> Key findings
          </label>
          <label style={{ fontSize: 13 }}>
            <input type="checkbox" checked={opts.messages} onChange={() => flip('messages')} /> Selected messages
          </label>
          <label style={{ fontSize: 13 }}>
            <input type="checkbox" checked={opts.attachments} onChange={() => flip('attachments')} /> Attachments
          </label>
          <label style={{ fontSize: 13 }}>
            <input type="checkbox" checked={opts.memory} onChange={() => flip('memory')} /> Visible memories
          </label>
          <label style={{ fontSize: 13 }}>
            Recent turns (max 20)
            <input
              type="number"
              min={1}
              max={20}
              value={opts.turns}
              onChange={(e) => setOpts((o) => ({ ...o, turns: Number(e.target.value) }))}
              style={{ width: '100%' }}
            />
          </label>
          <input value={opts.note} onChange={(e) => setOpts((o) => ({ ...o, note: e.target.value }))} placeholder="Note (optional)" />
          <div style={{ display: 'flex', gap: 8 }}>
            <button disabled={!target} onClick={() => onShare(target, opts)}>
              Share
            </button>
            <button onClick={onClose}>Cancel</button>
          </div>
        </div>
      </div>
    </div>
  );
}
