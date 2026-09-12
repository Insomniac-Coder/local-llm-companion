import { useState } from 'react';
import { optimizeModel } from '../services/api';
import { Badge } from '../ui/primitives';

// Stage 30 DAIO surfaces (§19): concise recommendation + rationale on demand.
const WORKLOADS = ['chat', 'reasoning', 'code', 'agent', 'vision', 'documents'];

export default function OptimizeCard({
  modelId,
  notify,
}: {
  modelId: string;
  notify: (k: 'info' | 'success' | 'error', t: string) => void;
}) {
  const [workload, setWorkload] = useState('code');
  const [policy, setPolicy] = useState('balanced');
  const [res, setRes] = useState<any>(null);
  const [showWhy, setShowWhy] = useState(false);

  const run = () => {
    optimizeModel(modelId, workload, policy)
      .then(setRes)
      .catch((e) => notify('error', e.message));
  };

  return (
    <div className="card" style={{ marginTop: 8 }}>
      <strong>Inference suggestions</strong>{' '}
      <Badge tone="info" title="Heuristic suggestions, not benchmark results or applied settings">
        Preview
      </Badge>
      <p style={{ fontSize: 14, lineHeight: 1.6, color: 'var(--text-secondary)', margin: '8px 0' }}>Estimates only. These suggestions do not change your runtime settings.</p>
      <div style={{ display: 'flex', gap: 6, marginTop: 6, flexWrap: 'wrap' }}>
        <select value={workload} onChange={(e) => setWorkload(e.target.value)} title="Workload">
          {WORKLOADS.map((w) => (
            <option key={w} value={w}>{w}</option>
          ))}
        </select>
        <select value={policy} onChange={(e) => setPolicy(e.target.value)} title="Policy">
          <option value="performance">Performance</option>
          <option value="balanced">Balanced</option>
          <option value="efficiency">Efficiency</option>
        </select>
        <button onClick={run}>Preview suggestions</button>
      </div>
      {res && (
        <div style={{ fontSize: 13, marginTop: 6 }}>
          <div>
            Backend: {res.placement.backend} · Placement: {res.placement.strategy} ({res.placement.gpu_layers_percent}% GPU)
          </div>
          <div>
            Projector: {res.placement.projector} · KV offload: {res.placement.kv_offload ? 'on' : 'off'} · Context:{' '}
            {res.placement.context}
          </div>
          <div>
            VRAM ~{res.placement.expected_vram_gb} GB · RAM ~{res.placement.expected_ram_gb} GB · Confidence:{' '}
            {res.placement.confidence}
          </div>
          <button className="ctx-toggle" style={{ marginTop: 4 }} onClick={() => setShowWhy((v) => !v)}>
            {showWhy ? '▾ Hide rationale' : '▸ Why this configuration?'}
          </button>
          {showWhy && (
            <ul style={{ margin: '4px 0', paddingLeft: 18 }}>
              {res.placement.rationale.map((r: string, i: number) => (
                <li key={i}>{r}</li>
              ))}
            </ul>
          )}
        </div>
      )}
    </div>
  );
}
