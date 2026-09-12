import { useEffect, useState } from 'react';
import { getDoctor, type DoctorCheck } from '../services/api';

// Stage 34 diagnostics card (§51): every subsystem as an honest row.
const DOT: Record<string, string> = { ok: '✓', warn: '⚠', fail: '✕' };

export default function DoctorCard({ notify }: { notify: (k: 'info' | 'error', t: string) => void }) {
  const [checks, setChecks] = useState<DoctorCheck[]>([]);
  const [status, setStatus] = useState('');

  const load = () => {
    getDoctor()
      .then((d) => {
        setChecks(d.checks);
        setStatus(d.status);
      })
      .catch((e) => notify('error', e.message));
  };
  useEffect(() => { load(); }, []);

  return (
    <div className="card">
      <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
        <strong>Doctor</strong>
        {status && <span style={{ fontSize: 12 }}>{DOT[status] ?? ''} {status}</span>}
        <span style={{ flex: 1 }} />
        <button onClick={load}>Re-run</button>
      </div>
      {checks.map((c) => (
        <div key={c.id} style={{ display: 'flex', gap: 8, fontSize: 13, marginTop: 4 }}>
          <span title={c.status}>{DOT[c.status] ?? '?'}</span>
          <span style={{ minWidth: 140 }}>{c.label}</span>
          <span style={{ flex: 1, color: 'var(--text-secondary)' }}>{c.detail}</span>
        </div>
      ))}
    </div>
  );
}
