import { useState } from 'react';
import { optimizeModel } from '../services/api';
import { Badge, Button } from '../ui/primitives';
import { Icon } from '../ui/Icon';

// Runtime suggestions: a concise recommendation with its rationale on demand.
const WORKLOADS = ['chat', 'reasoning', 'code', 'agent', 'vision', 'documents'];
const cap = (value: string) => value[0].toUpperCase() + value.slice(1);

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
  const [busy, setBusy] = useState(false);

  const run = () => {
    setBusy(true);
    optimizeModel(modelId, workload, policy)
      .then(setRes)
      .catch((e) => notify('error', e.message))
      .finally(() => setBusy(false));
  };

  return (
    <div className="tool-card">
      <header><strong>Inference suggestions</strong><Badge title="Heuristic suggestions, not benchmark results or applied settings">Preview</Badge></header>
      <p>Estimates only. These suggestions do not change your runtime settings.</p>
      <div className="controls">
        <select value={workload} onChange={(e) => setWorkload(e.target.value)} aria-label="Workload">
          {WORKLOADS.map((w) => <option key={w} value={w}>{cap(w)}</option>)}
        </select>
        <select value={policy} onChange={(e) => setPolicy(e.target.value)} aria-label="Policy">
          <option value="performance">Performance</option>
          <option value="balanced">Balanced</option>
          <option value="efficiency">Efficiency</option>
        </select>
        <Button size="sm" loading={busy} onClick={run} style={{ height: 34 }}>Preview suggestions</Button>
      </div>
      {res && (
        <>
          <dl className="kv">
            <dt>Backend</dt><dd>{res.placement.backend}</dd>
            <dt>Placement</dt><dd>{res.placement.strategy} · {res.placement.gpu_layers_percent}% GPU</dd>
            <dt>Projector</dt><dd>{res.placement.projector}</dd>
            <dt>KV offload</dt><dd>{res.placement.kv_offload ? 'On' : 'Off'}</dd>
            <dt>Context</dt><dd>{res.placement.context}</dd>
            <dt>Expected memory</dt><dd>VRAM ~{res.placement.expected_vram_gb} GB · RAM ~{res.placement.expected_ram_gb} GB</dd>
            <dt>Confidence</dt><dd>{res.placement.confidence}</dd>
          </dl>
          <button type="button" className="activity-disclosure" aria-expanded={showWhy} onClick={() => setShowWhy((v) => !v)}>
            <Icon name="chevronRight" size={13} /> Why this configuration?
          </button>
          {showWhy && <ul className="bullet-list">{res.placement.rationale.map((r: string, i: number) => <li key={i}>{r}</li>)}</ul>}
        </>
      )}
    </div>
  );
}
