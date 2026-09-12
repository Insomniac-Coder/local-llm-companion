import { useState } from 'react';
import { getSettings, putSettings, recommendContext, type Recommendation } from '../services/api';
import { Badge } from '../ui/primitives';

const WORKLOADS = ['chat', 'docs', 'coding', 'agent', 'vision'];
const PROFILES = ['efficient', 'balanced', 'maximum'];

function verdictColor(v: string): string {
  if (v === 'recommended') return 'var(--success)';
  if (v === 'supported') return 'var(--text-secondary)';
  if (v === 'risky') return 'var(--warning)';
  return 'var(--error)';
}

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

  const fetch = () => {
    recommendContext(modelId, workload, profile)
      .then(setRec)
      .catch((e) => notify('error', e.message));
  };

  const apply = (ctx: number) => {
    getSettings()
      .then((s) => {
        s.inference.context_size = ctx;
        return putSettings(s);
      })
      .then(() => notify('success', `Context set to ${ctx} tokens. Takes effect on next inference start.`))
      .catch((e) => notify('error', e.message));
  };

  return (
    <div className="card">
      <strong>Recommended context</strong>
      <div style={{ display: 'flex', gap: 8, marginTop: 8, flexWrap: 'wrap' }}>
        <select value={workload} onChange={(e) => setWorkload(e.target.value)} aria-label="Workload">
          {WORKLOADS.map((w) => (
            <option key={w} value={w}>{w}</option>
          ))}
        </select>
        <select value={profile} onChange={(e) => setProfile(e.target.value)} aria-label="Profile">
          {PROFILES.map((p) => (
            <option key={p} value={p}>{p}</option>
          ))}
        </select>
        <button onClick={fetch}>Get recommendation</button>
      </div>
      {rec && (
        <div style={{ marginTop: 8, fontSize: 13 }}>
          <div>
            <strong>{(rec.recommended_ctx / 1024).toFixed(0)}K tokens</strong>{' '}
            <Badge tone="ok">Recommended</Badge>
            {' · '}confidence: {rec.confidence}
            {' · '}weights ~{rec.weights_gb} GB, KV ~{rec.kv_gb} GB, headroom ~{rec.headroom_gb} GB
          </div>
          <ul style={{ margin: '6px 0', paddingLeft: 18 }}>
            {rec.rationale.map((r, i) => (
              <li key={i}>{r}</li>
            ))}
          </ul>
          <div style={{ display: 'flex', gap: 6, flexWrap: 'wrap', marginTop: 6 }}>
            {rec.candidates.map((c) => (
              <button
                key={c.ctx}
                title={`${c.ctx} tokens · KV ~${c.kv_gb} GB · total ~${c.total_gb} GB — ${c.verdict}`}
                disabled={c.verdict === 'unavailable'}
                onClick={() => apply(c.ctx)}
                style={{ borderColor: verdictColor(c.verdict) }}
              >
                {(c.ctx / 1024).toFixed(0)}K · {c.verdict}
              </button>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}
