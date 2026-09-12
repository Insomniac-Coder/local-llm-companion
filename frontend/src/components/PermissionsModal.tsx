import { useEffect, useState } from 'react';
import { Button } from '../ui/primitives';
import { getSettings } from '../services/api';
import { AUTO_POLICY_DESCRIPTION, PROJECT_BOUNDARY_DESCRIPTION, SEARCH_PERMISSION_DESCRIPTION } from './permissionCopy';

export function PermissionSummary({ settings }: { settings: any }) {
  return <>
    <p style={{ fontSize: 14, marginTop: 12 }}>Approval policy: <strong>{settings.agent?.autonomous_enabled ? 'Auto — no approval prompts' : 'Ask — approve actions'}</strong></p>
    <p style={{ fontSize: 14, lineHeight: 1.7, color: 'var(--text-secondary)' }}>Ask mode requests approval for agent actions. {AUTO_POLICY_DESCRIPTION}</p>
    <p style={{ fontSize: 14, lineHeight: 1.7, color: 'var(--text-secondary)' }}>{PROJECT_BOUNDARY_DESCRIPTION} Use Auto only for tasks and projects you trust.</p>
    <p style={{ fontSize: 14, lineHeight: 1.7, color: 'var(--text-secondary)' }}>{SEARCH_PERMISSION_DESCRIPTION}</p>
    <p style={{ fontSize: 14 }}>Agent search: <strong>{settings.search?.autonomous === 'deny' ? 'Not allowed' : settings.search?.autonomous === 'allow' ? 'Allowed when Search is enabled' : 'Ask unless Auto mode is on'}</strong></p>
  </>;
}

// Design guide §15: scoped-permission summary modal. Approvals themselves
// still happen inline where tools run; this surface explains policy.
export default function PermissionsModal({
  onClose,
  onOpenSettings,
}: {
  onClose: () => void;
  onOpenSettings: () => void;
}) {
  const [s, setS] = useState<any>(null);
  useEffect(() => {
    getSettings().then(setS).catch(() => setS(null));
  }, []);
  return (
    <div className="modal-backdrop" onClick={onClose} role="presentation">
      <div className="modal" role="dialog" aria-modal="true" aria-label="Permissions" onClick={(e) => e.stopPropagation()}>
        <strong>Permissions</strong>
        {!s && <div style={{ fontSize: 13 }}>Loading policy…</div>}
        {s && <PermissionSummary settings={s} />}
        <div className="perm-actions">
          <Button onClick={() => { onClose(); onOpenSettings(); }}>Open Settings</Button>
          <Button variant="ghost" onClick={onClose}>Close</Button>
        </div>
      </div>
    </div>
  );
}
