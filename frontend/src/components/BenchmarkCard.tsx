import { useState } from 'react';
import { runBenchmark, type BenchmarkResult } from '../services/api';
import { Button, Section } from '../ui/primitives';

// Benchmark: a short timed generation through the running model. Honest when idle.
export default function BenchmarkCard({ notify }: { notify: (k: 'info' | 'success' | 'error', t: string) => void }) {
  const [res, setRes] = useState<BenchmarkResult | null>(null);
  const [busy, setBusy] = useState(false);

  const run = () => {
    setBusy(true);
    runBenchmark('', 64)
      .then((r) => {
        setRes(r);
        notify('success', `Benchmark: ${r.generation_tps} tok/s on ${r.model || 'the running model'}.`);
      })
      .catch((e) => notify('info', e.message))
      .finally(() => setBusy(false));
  };

  return (
    <div className="panel">
      <Section title="Benchmark" icon="zap" actions={<Button size="sm" icon="play" loading={busy} onClick={run}>{busy ? 'Running…' : 'Run benchmark'}</Button>}>
        <p className="help">A short timed generation through the running model. Needs a loaded model.</p>
        {res && (
          <div className="bench">
            <div className="bench-number"><strong>{res.generation_tps}</strong><span>tok/s</span></div>
            <dl className="kv">
              <dt>Model</dt><dd>{res.model || '—'}</dd>
              <dt>Prompt</dt><dd>{res.prompt_tokens} tokens</dd>
              <dt>Generated</dt><dd>{res.generated_tokens} tokens in {(res.total_ms / 1000).toFixed(1)} s</dd>
              <dt>Context limit</dt><dd>{res.context_limit.toLocaleString()}</dd>
            </dl>
          </div>
        )}
      </Section>
    </div>
  );
}
