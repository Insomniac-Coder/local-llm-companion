import { useEffect, useState } from 'react';
import { getLoadProgress, cancelLoad, type LoadProgress as LP } from '../services/api';

// Stage 21: staged model-load progress with real stages + cancel (§174 rule).
export default function LoadProgress({ active, notify }: { active: boolean; notify: (k: 'info' | 'error', t: string) => void }) {
  const [p, setP] = useState<LP | null>(null);
  useEffect(() => {
    if (!active) {
      setP(null);
      return;
    }
    let stop = false;
    const tick = () => {
      getLoadProgress()
        .then((v) => {
          if (!stop) setP(v);
        })
        .catch(() => {});
    };
    tick();
    const t = setInterval(tick, 1500);
    return () => {
      stop = true;
      clearInterval(t);
    };
  }, [active]);
  if (!active || !p || p.stage === 'idle' || p.stage === 'ready') return null;
  return (
    <div className="card" role="status" style={{ margin: '8px 16px 0', borderColor: 'var(--info)' }}>
      <div>
        <strong>Loading {p.model_id || 'model'}…</strong> <span style={{ fontSize: 12 }}>({p.stage})</span>
      </div>
      <div style={{ fontSize: 13 }}>{p.detail || 'Working…'}</div>
      {p.stage === 'error' && <div className="approval">{p.detail}</div>}
      {(p.stage === 'validating' || p.stage === 'loading') && (
        <button
          style={{ marginTop: 6 }}
          onClick={() => cancelLoad().then(() => notify('info', 'Load cancelled.')).catch((e) => notify('error', e.message))}
        >
          Cancel
        </button>
      )}
    </div>
  );
}
