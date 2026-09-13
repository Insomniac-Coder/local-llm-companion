import { useState } from 'react';
import { getSettings, putSettings, recommendContext, type Recommendation } from '../services/api';
import { Button } from '../ui/primitives';

const WORKLOADS = ['chat', 'docs', 'coding', 'agent', 'vision'];
const PROFILES = ['efficient', 'balanced', 'maximum'];
const cap = (value: string) => value[0].toUpperCase() + value.slice(1);

export default function RecommendCard({
  modelId,
  notify,
}: {
  modelId: string;
  notify: (kind: 'info' | 'success' | 'warning' | 'error', text: string) => void;
}) {
  const [workload, setWorkload] = useState('coding');
  const [profile, setProfile] = useState('balanced');
  const [rec, setRec] = useState<Recommendation | null>(null);
  const [busy, setBusy] = useState(false);

  const fetch = () => {
    setBusy(true);
    recommendContext(modelId, workload, profile)
      .then(setRec)
      .catch((e) => notify('error', e.message))
      .finally(() => setBusy(false));
  };

  const apply = (ctx: number) => {
    getSettings()
      .then((s) => {
        s.inference.context_size = ctx;
        return putSettings(s);
      })
      .then(() => notify('success', `Context set to ${ctx.toLocaleString()} tokens. It takes effect on the next model load.`))
      .catch((e) => notify('error', e.message));
  };

  return (
    <div className="tool-card">
      <header><strong>Recommended context</strong></header>
      <p>How large a context window fits this model on your machine for a type of work.</p>
      <div className="controls">
        <select value={workload} onChange={(e) => setWorkload(e.target.value)} aria-label="Workload">
          {WORKLOADS.map((w) => <option key={w} value={w}>{cap(w)}</option>)}
        </select>
        <select value={profile} onChange={(e) => setProfile(e.target.value)} aria-label="Profile">
          {PROFILES.map((p) => <option key={p} value={p}>{cap(p)}</option>)}
        </select>
        <Button size="sm" loading={busy} onClick={fetch} style={{ height: 34 }}>Get recommendation</Button>
      </div>
      {rec && (
        <>
          <div className="rec-headline">
            <strong>{(rec.recommended_ctx / 1024).toFixed(0)}K</strong>
            <span>tokens · {rec.confidence} confidence · weights ~{rec.weights_gb} GB · KV ~{rec.kv_gb} GB · headroom ~{rec.headroom_gb} GB</span>
          </div>
          <ul className="bullet-list">{rec.rationale.map((r, i) => <li key={i}>{r}</li>)}</ul>
          <div className="rec-candidates">
            {rec.candidates.map((c) => (
              <button
                type="button"
                key={c.ctx}
                className={`rec-candidate ${c.verdict}`}
                title={`${c.ctx} tokens · KV ~${c.kv_gb} GB · total ~${c.total_gb} GB — ${c.verdict}. Click to save for the next load.`}
                disabled={c.verdict === 'unavailable'}
                onClick={() => apply(c.ctx)}
              >
                {(c.ctx / 1024).toFixed(0)}K<small>{c.verdict}</small>
              </button>
            ))}
          </div>
        </>
      )}
    </div>
  );
}
