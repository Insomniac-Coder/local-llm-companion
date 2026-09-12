import { useEffect, useState } from 'react';
import { elapsedLabel, elapsedSeconds } from '../services/workElapsed';

export default function WorkStatus({ active, label, waiting = false, startedAt }: { active: boolean; label: string; waiting?: boolean; startedAt?: number | null }) {
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    if (!active) return;
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [active, startedAt]);
  if (!active) return null;
  const elapsed = elapsedSeconds(startedAt, now);
  return <div className={`work-status${waiting ? ' waiting' : ' live'}`}>
    <span className="work-status-dots" aria-hidden="true"><i/><i/><i/></span>
    <span role="status" aria-live="polite" aria-atomic="true">{label}</span>
    <span className="work-status-time" aria-hidden="true" title={elapsed == null ? 'Activity start time unavailable' : 'Elapsed time since this request or run started, including waits'}>{elapsedLabel(elapsed)}</span>
  </div>;
}
