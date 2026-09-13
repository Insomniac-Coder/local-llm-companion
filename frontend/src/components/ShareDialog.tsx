import { useState } from 'react';
import { Button, Dialog } from '../ui/primitives';

// Share picker: explicit include checkboxes, nothing is sent implicitly.
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
  return (
    <Dialog
      title="Share context"
      description="Copy selected context from this session into another one. Only what you tick is included."
      icon="share"
      onClose={onClose}
      footer={<>
        <Button variant="ghost" onClick={onClose}>Cancel</Button>
        <Button icon="share" disabled={!target} onClick={() => onShare(target, opts)}>Share</Button>
      </>}
    >
      <div className="form-stack">
        <label className="field">
          <span>Destination session</span>
          <select value={target} onChange={(e) => setTarget(e.target.value)}>
            <option value="">Choose a session…</option>
            {targets.map((t) => <option key={t.id} value={t.id}>{t.label}</option>)}
          </select>
        </label>
        <div className="field">
          <span>Include</span>
          <div className="checks">
            <label className="check"><input type="checkbox" checked={opts.summary} onChange={() => flip('summary')} /> Key findings</label>
            <label className="check"><input type="checkbox" checked={opts.messages} onChange={() => flip('messages')} /> Selected messages</label>
            <label className="check"><input type="checkbox" checked={opts.attachments} onChange={() => flip('attachments')} /> Attachments</label>
            <label className="check"><input type="checkbox" checked={opts.memory} onChange={() => flip('memory')} /> Visible memories</label>
          </div>
        </div>
        <label className="field">
          <span>Recent turns</span>
          <input type="number" min={1} max={20} value={opts.turns} disabled={!opts.messages} onChange={(e) => setOpts((o) => ({ ...o, turns: Number(e.target.value) }))} />
          <span className="field-help">Up to 20 of the most recent turns.</span>
        </label>
        <label className="field">
          <span>Note <em className="muted" style={{ fontStyle: 'normal', fontWeight: 400 }}>(optional)</em></span>
          <input value={opts.note} onChange={(e) => setOpts((o) => ({ ...o, note: e.target.value }))} placeholder="Why you’re sharing this" />
        </label>
      </div>
    </Dialog>
  );
}
