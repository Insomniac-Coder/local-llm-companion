import { useEffect, useState } from 'react';
import { getSetupStatus, type SetupStatus } from '../services/api';

// Stage 38 first-run wizard (§88): detect → binary → model → test → chat.
export default function SetupWizard({ notify }: { notify: (k: 'info' | 'error', t: string) => void }) {
  const [st, setSt] = useState<SetupStatus | null>(null);

  const load = () => getSetupStatus().then(setSt).catch((e) => notify('error', e.message));
  useEffect(() => { load(); }, []);

  if (!st || !st.needs_setup) return null;
  return (
    <div className="card" style={{ borderColor: 'var(--info)' }}>
      <strong>Welcome — let's set up your local AI</strong>
      {st.steps.map((s, i) => (
        <div key={s.id} style={{ display: 'flex', gap: 8, fontSize: 13, marginTop: 4 }}>
          <span>{s.done ? '✓' : `${i + 1}.`}</span>
          <span style={{ flex: 1 }}>{s.label}</span>
          {!s.done && <span style={{ color: 'var(--warning)' }}>todo</span>}
        </div>
      ))}
      <div style={{ fontSize: 12, color: 'var(--text-secondary)', marginTop: 6 }}>
        {!st.binary_found && 'Install llama-server: download a llama.cpp release, place llama-server(.exe) on PATH or models/bin/. '}
        {st.binary_found && st.models_with_gguf === 0 && 'Download a GGUF below (paste a HuggingFace resolve URL), then Scan. '}
        {st.models_with_gguf > 0 && !st.inference_running && 'Load a model and press Start inference in the System tab. '}
      </div>
      <button style={{ marginTop: 6 }} onClick={load}>Re-check</button>
    </div>
  );
}
