import { useEffect, useState } from 'react';
import { Button, Dialog } from '../ui/primitives';
import { Icon } from '../ui/Icon';
import { getSettings } from '../services/api';
import { PERMISSION_MODE_DESCRIPTIONS, PERMISSION_MODE_LABELS, PROJECT_BOUNDARY_DESCRIPTION, SEARCH_PERMISSION_DESCRIPTION } from './permissionCopy';

export function PermissionSummary({ settings }: { settings: any }) {
  const mode = (settings.agent?.permission_mode in PERMISSION_MODE_LABELS ? settings.agent.permission_mode : settings.agent?.autonomous_enabled ? 'auto' : 'ask') as keyof typeof PERMISSION_MODE_LABELS;
  const auto = mode === 'auto';
  const searchPolicy = settings.search?.autonomous === 'deny' ? 'Not allowed' : settings.search?.autonomous === 'allow' ? 'Allowed when Search is enabled' : 'Ask unless Auto mode is on';
  return <>
    <dl className="policy-status">
      <dt>Permission mode</dt><dd className={auto ? 'auto' : ''}>{PERMISSION_MODE_LABELS[mode]} — {PERMISSION_MODE_DESCRIPTIONS[mode]}</dd>
      <dt>Agent search</dt><dd>{searchPolicy}</dd>
    </dl>
    <ul className="policy-list">
      <li><Icon name="shield" size={16} /><span>{(Object.keys(PERMISSION_MODE_LABELS) as (keyof typeof PERMISSION_MODE_LABELS)[]).map((option) => `${PERMISSION_MODE_LABELS[option]}: ${PERMISSION_MODE_DESCRIPTIONS[option]}`).join(' ')}</span></li>
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
