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
  return (
    <div className="pop-anchor" style={{ display: 'block' }}>
      <button type="button" className="sb-attention" aria-haspopup="menu" aria-expanded={open} onClick={() => setOpen((value) => !value)}>
        <Icon name="alert" size={15} />
        <span>{count} {count === 1 ? 'session needs' : 'sessions need'} attention</span>
        <Icon name="chevronRight" size={14} />
      </button>
      <Popover open={open} onClose={() => setOpen(false)} label="Sessions needing attention" side="bottom" align="start" className="attention-pop" autoFocus={false}>
        <PopLabel>
          <span>Interrupted sessions</span>
        </PopLabel>
        {recovery.busy.length > 0 && <p className="attention-note">{recovery.busy.length} {recovery.busy.length === 1 ? 'session still has' : 'sessions still have'} live work.</p>}
        {recovery.stale.slice(0, 6).map((session) => (
          <div key={session.id} className="attention-item">
            <strong title={session.title}>{session.title}</strong>
            <small>Last used with {session.last_model || 'an unknown model'}</small>
            <div className="attention-actions">
              <Button size="sm" onClick={() => { setOpen(false); onResume(session.id); }}>Resume</Button>
              <IconButton icon="x" label={`Discard the interrupted state of ${session.title}`} size="sm" tipSide="left" onClick={() => onDiscard(session.id)} />
            </div>
          </div>
        ))}
        <p className="attention-note">Resume reopens the saved history for the model that is loaded now. Discard only clears the interrupted marker; nothing is deleted.</p>
        <div style={{ display: 'flex', justifyContent: 'flex-end', padding: '2px 4px 4px' }}>
          <Button size="sm" variant="ghost" onClick={() => { setOpen(false); onDismiss(); }}>Hide for now</Button>
        </div>
      </Popover>
    </div>
  );
}
