import { useEffect, useState } from 'react';
import { Button, Dialog } from '../ui/primitives';
import { Icon } from '../ui/Icon';
import { getSettings } from '../services/api';
import { AUTO_POLICY_DESCRIPTION, PROJECT_BOUNDARY_DESCRIPTION, SEARCH_PERMISSION_DESCRIPTION } from './permissionCopy';

export function PermissionSummary({ settings }: { settings: any }) {
  const auto = !!settings.agent?.autonomous_enabled;
  const searchPolicy = settings.search?.autonomous === 'deny' ? 'Not allowed' : settings.search?.autonomous === 'allow' ? 'Allowed when Search is enabled' : 'Ask unless Auto mode is on';
  return <>
    <dl className="policy-status">
      <dt>Approval policy</dt><dd className={auto ? 'auto' : ''}>{auto ? 'Auto — no approval prompts' : 'Ask — approve actions'}</dd>
      <dt>Agent search</dt><dd>{searchPolicy}</dd>
    </dl>
    <ul className="policy-list">
      <li><Icon name="shield" size={16} /><span>Ask mode requests approval for agent actions. {AUTO_POLICY_DESCRIPTION}</span></li>
      <li><Icon name="folder" size={16} /><span>{PROJECT_BOUNDARY_DESCRIPTION} Use Auto only for tasks and projects you trust.</span></li>
      <li><Icon name="globe" size={16} /><span>{SEARCH_PERMISSION_DESCRIPTION}</span></li>
    </ul>
  </>;
}

// Scoped-permission summary. Approvals themselves still happen inline where
// tools run; this surface explains the policy.
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
    <Dialog
      title="Permissions"
      description="What the agent may do without asking you first."
      icon="shield"
      onClose={onClose}
      footer={<>
        <Button variant="ghost" onClick={onClose}>Close</Button>
        <Button icon="sliders" onClick={() => { onClose(); onOpenSettings(); }}>Open settings</Button>
      </>}
    >
      {!s && <p>Loading policy…</p>}
      {s && <PermissionSummary settings={s} />}
    </Dialog>
  );
}
