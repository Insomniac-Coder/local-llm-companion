import { useEffect, useState } from 'react';
import { checkCompatibility, prepareContext, type CompatInfo, type PrepStage } from '../services/api';

/**
 * Informational model-switch notice. The next request rebuilds bounded
 * history automatically; checking readiness is optional, never a send gate.
 */
export default function PrepareBanner({
  convId,
  convTitle,
  lastModel,
  loadedModel,
  lastModelAvailable,
  onPrepared,
  onSwitchBack,
  notify,
}: {
  convId: string;
  convTitle: string;
  lastModel: string;
  loadedModel: string;
  lastModelAvailable: boolean;
  onPrepared: () => void;
  onSwitchBack: () => void;
  notify: (kind: 'info' | 'success' | 'warning' | 'error', text: string) => void;
}) {
  const [compat, setCompat] = useState<CompatInfo | null>(null);
  const [stages, setStages] = useState<PrepStage[]>([]);
  const [running, setRunning] = useState(false);

  useEffect(() => {
    checkCompatibility(convId, loadedModel).then(setCompat).catch(() => setCompat(null));
  }, [convId, loadedModel]);

  const prepare = () => {
    setRunning(true);
    setStages([]);
    const ctl = new AbortController();
    prepareContext(convId, loadedModel, (s) => setStages((prev) => [...prev, s]), ctl.signal)
      .then((r) => {
        setRunning(false);
        if (r.ready) {
          notify('success', `Session checked — context will be rebuilt on the next reply.`);
          onPrepared();
        }
      })
      .catch((e: any) => {
        setRunning(false);
        if (e?.name !== 'AbortError') notify('error', e?.message ?? 'Preparation failed.');
      });
  };

  return (
    <div className="card" role="status" style={{ margin: '8px 16px 0' }}>
      <div><strong>Model changed</strong></div>
      <div style={{ fontSize: 13 }}>
        “{convTitle}” was last prepared for <strong>{lastModel || 'no model'}</strong>, now using <strong>{loadedModel}</strong>.
        {' '}Continue chatting normally. The new model will rebuild its context from saved messages on your next reply; the previous model’s cache is not reused.
      </div>
      {compat && compat.warnings.length > 0 && (
        <div style={{ marginTop: 6 }}>
          {compat.warnings.map((w, i) => (
            <div key={i} className="approval" style={{ marginTop: 4, fontSize: 12 }}>{w}</div>
          ))}
        </div>
      )}
      {stages.length > 0 && (
        <div style={{ marginTop: 6, fontSize: 13 }}>
          {stages.map((s, i) => (
            <div key={i}>
              {s.status === 'done' ? '✓' : '●'} {s.stage} — {s.detail}
            </div>
          ))}
        </div>
      )}
      <div style={{ display: 'flex', gap: 8, marginTop: 8 }}>
        <button disabled={running} onClick={prepare}>
          {running ? 'Checking session…' : 'Check session readiness'}
        </button>
        <button disabled={running || !lastModelAvailable} onClick={onSwitchBack} title={!lastModelAvailable ? 'This model is no longer installed' : 'Load the previous model'}>
          {lastModelAvailable ? 'Switch Back' : 'Previous Model Unavailable'}
        </button>
      </div>
    </div>
  );
}
