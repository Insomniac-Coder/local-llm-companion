import type { RecoveryInfo } from '../services/api';

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
  const count = recovery.stale.length + recovery.busy.length;
  if (count === 0) return null;
  return (
    <div className="recovery-shell" role="status">
      <details className="recovery-strip">
        <summary>
          <i aria-hidden="true" />
          <strong>{count} previous {count === 1 ? 'session needs' : 'sessions need'} attention</strong>
          <span>Review</span>
        </summary>
        <div className="recovery-list">
          {recovery.busy.length > 0 && <p>{recovery.busy.length} {recovery.busy.length === 1 ? 'session still has' : 'sessions still have'} live work.</p>}
          {recovery.stale.slice(0, 5).map((session) => (
            <div key={session.id}>
              <span><strong>{session.title}</strong><small>Last used {session.last_model}</small></span>
              <button onClick={() => onResume(session.id)}>Resume</button>
              <button className="quiet" onClick={() => onDiscard(session.id)}>Discard</button>
            </div>
          ))}
        </div>
      </details>
      <button className="ctx-toggle" onClick={onDismiss} aria-label="Dismiss recovery notice">×</button>
    </div>
  );
}
