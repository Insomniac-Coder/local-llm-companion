import { useEffect, useState } from 'react';
import { getDoctor, type DoctorCheck } from '../services/api';
import { Badge, Button, Section } from '../ui/primitives';
import { Icon, type IconName } from '../ui/Icon';

// Health checks: every subsystem as an honest row.
const ICON: Record<string, IconName> = { ok: 'checkCircle', warn: 'alert', fail: 'alertCircle' };
const TONE: Record<string, 'ok' | 'warn' | 'err'> = { ok: 'ok', warn: 'warn', fail: 'err' };

export default function DoctorCard({ notify }: { notify: (k: 'info' | 'error', t: string) => void }) {
  const [checks, setChecks] = useState<DoctorCheck[]>([]);
  const [status, setStatus] = useState('');
  const [running, setRunning] = useState(false);

  const load = () => {
    setRunning(true);
    getDoctor()
      .then((d) => {
        setChecks(d.checks);
        setStatus(d.status);
      })
      .catch((e) => notify('error', e.message))
      .finally(() => setRunning(false));
  };
  useEffect(() => { load(); }, []);

  const failing = checks.filter((check) => check.status !== 'ok').length;
  return (
    <div className="panel">
      <Section
        title="Health checks"
        icon="shieldCheck"
        meta={checks.length ? failing ? `${failing} of ${checks.length} need a look` : `All ${checks.length} passed` : undefined}
        actions={<>{status && <Badge tone={TONE[status] ?? 'neutral'}>{status === 'ok' ? 'Healthy' : status === 'warn' ? 'Warnings' : 'Problems'}</Badge>}<Button size="sm" variant="ghost" icon="refresh" loading={running} onClick={load}>Run again</Button></>}
      >
        <ul className="check-list">
          {checks.map((c) => (
            <li key={c.id} className={c.status}>
              <Icon name={ICON[c.status] ?? 'help'} size={16} />
              <span className="check-label">{c.label}</span>
              <span className="check-detail">{c.detail}</span>
            </li>
          ))}
        </ul>
      </Section>
    </div>
  );
}
