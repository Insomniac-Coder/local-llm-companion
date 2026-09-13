import { useEffect, useState } from 'react';
import { getSetupStatus, type SetupStatus } from '../services/api';
import { Button } from '../ui/primitives';
import { Icon } from '../ui/Icon';

// First-run checklist: detect → runtime → model → test → chat.
export default function SetupWizard({ notify }: { notify: (k: 'info' | 'error', t: string) => void }) {
  const [st, setSt] = useState<SetupStatus | null>(null);

  const load = () => getSetupStatus().then(setSt).catch((e) => notify('error', e.message));
  useEffect(() => { load(); }, []);

  if (!st || !st.needs_setup) return null;
  const next = st.steps.findIndex((step) => !step.done);
  return (
    <section className="panel setup-panel" aria-label="Setup">
      <div className="panel-head">
        <div>
          <h2>Set up your local AI</h2>
          <p>
            {!st.binary_found && 'Install llama-server: download a llama.cpp release and place llama-server(.exe) on PATH or in models/bin. '}
            {st.binary_found && st.models_with_gguf === 0 && 'Download a GGUF model below, or put one in the models folder and scan. '}
            {st.models_with_gguf > 0 && !st.inference_running && 'Load a model from the machine panel to finish setup. '}
          </p>
        </div>
        <Button size="sm" variant="ghost" icon="refresh" onClick={() => void load()}>Check again</Button>
      </div>
      <ol className="setup-steps">
        {st.steps.map((s, i) => (
          <li key={s.id} className={s.done ? 'done' : i === next ? 'next' : ''}>
            <span className="step-mark">{s.done ? <Icon name="check" size={12} strokeWidth={2.4} /> : i + 1}</span>
            {s.label}
          </li>
        ))}
      </ol>
    </section>
  );
}
