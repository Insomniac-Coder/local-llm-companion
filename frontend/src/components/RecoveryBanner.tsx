import { useState } from 'react';
import type { RecoveryInfo } from '../services/api';
import { Button, IconButton, PopLabel, Popover } from '../ui/primitives';
import { Icon } from '../ui/Icon';

/** Interrupted sessions belong to the session list, not to every page: a
 *  compact sidebar entry that opens their Resume/Discard choices. */
export default function RecoveryBanner({
  recovery,
  onResume,
  onDiscard,
  onDismiss,
}: {
  recovery: RecoveryInfo;
  onResume: (id: string) => void;
  onDiscard: (id: string) => void;
  onDismiss: () => void;
}) {
  const [open, setOpen] = useState(false);
  const count = recovery.stale.length + recovery.busy.length;
  if (count === 0) return null;
  // A session whose last model differs from the loaded one is not broken:
  // its history is intact and rebuilds on the next message. Only live work
  // left behind is worth the alarm wording.
  const summary = recovery.busy.length > 0
    ? `${count} ${count === 1 ? 'session needs' : 'sessions need'} attention`
    : `${count} ${count === 1 ? 'session used' : 'sessions used'} another model`;
  return (
    <div className="pop-anchor" style={{ display: 'block' }}>
      <button type="button" className="sb-attention" aria-haspopup="menu" aria-expanded={open} onClick={() => setOpen((value) => !value)}>
        <Icon name={recovery.busy.length > 0 ? 'alert' : 'info'} size={15} />
        <span>{summary}</span>
        <Icon name="chevronRight" size={14} />
      </button>
      <Popover open={open} onClose={() => setOpen(false)} label="Sessions from another model" side="bottom" align="start" className="attention-pop" autoFocus={false}>
        <PopLabel>
          <span>{recovery.busy.length > 0 ? 'Interrupted sessions' : 'Last used with another model'}</span>
        </PopLabel>
        {recovery.busy.length > 0 && <p className="attention-note">{recovery.busy.length} {recovery.busy.length === 1 ? 'session still has' : 'sessions still have'} live work.</p>}
        {recovery.stale.slice(0, 6).map((session) => (
          <div key={session.id} className="attention-item">
            <strong title={session.title}>{session.title}</strong>
            <small>Last used with {session.last_model || 'an unknown model'}</small>
            <div className="attention-actions">
              <Button size="sm" onClick={() => { setOpen(false); onResume(session.id); }}>Resume</Button>
              <IconButton icon="x" label={`Clear the other-model marker of ${session.title}`} size="sm" tipSide="left" onClick={() => onDiscard(session.id)} />
            </div>
          </div>
        ))}
        <p className="attention-note">Resume reopens the saved history with the model that is loaded now; the context is rebuilt on the next message and nothing is lost. Clearing only removes the marker.</p>
        <div style={{ display: 'flex', justifyContent: 'flex-end', padding: '2px 4px 4px' }}>
          <Button size="sm" variant="ghost" onClick={() => { setOpen(false); onDismiss(); }}>Hide for now</Button>
        </div>
      </Popover>
    </div>
  );
}
