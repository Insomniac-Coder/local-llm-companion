import { useState } from 'react';
import { runBenchmark, type BenchmarkResult } from '../services/api';

// Stage 38 benchmark card (§89): timed generation, honest when idle.
export default function BenchmarkCard({ notify }: { notify: (k: 'info' | 'success' | 'error', t: string) => void }) {
  const [res, setRes] = useState<BenchmarkResult | null>(null);
  const [busy, setBusy] = useState(false);

  const run = () => {
    setBusy(true);
    runBenchmark('', 64)
      .then((r) => {
        setRes(r);
        notify('success', `Benchmark: ${r.generation_tps} tok/s on ${r.model || 'sidecar'}.`);
      })
      .catch((e) => notify('info', e.message))
      .finally(() => setBusy(false));
  };

  return (
    <div className="card">
      <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
        <strong>Benchmark</strong>
        <span style={{ flex: 1 }} />
        <button disabled={busy} onClick={run}>{busy ? 'Running…' : 'Run benchmark'}</button>
      </div>
      <div style={{ fontSize: 12, color: 'var(--text-secondary)', marginTop: 4 }}>
        Short timed generation through the live sidecar. Needs inference running.
      </div>
      {res && (
        <div style={{ fontSize: 13, marginTop: 6 }}>
          <div>Model: {res.model || '—'}</div>
          <div>Prompt: {res.prompt_tokens} tok · Generated: {res.generated_tokens} tok in {(res.total_ms / 1000).toFixed(1)}s</div>
          <div><strong>{res.generation_tps} tok/s</strong> · context limit {res.context_limit}</div>
        </div>
      )}
    </div>
  );
}
