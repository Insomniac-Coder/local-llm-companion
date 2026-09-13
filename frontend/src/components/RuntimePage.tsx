import { inferenceStatus, inferenceStop, systemInfo, type InferenceStatus } from '../services/api';
import DoctorCard from './DoctorCard';
import BenchmarkCard from './BenchmarkCard';
import { Button, Lamp, Notice, Section } from '../ui/primitives';

type Notify = (kind: 'info' | 'success' | 'warning' | 'error', text: string) => void;

export default function RuntimePage({
  inf,
  sys,
  modelId,
  modelName,
  backendUp,
  notify,
  onStart,
  setInf,
  setSys,
}: {
  inf: InferenceStatus | null;
  sys: unknown;
  modelId: string;
  modelName?: string;
  backendUp: boolean | null;
  notify: Notify;
  onStart: () => void;
  setInf: (value: InferenceStatus) => void;
  setSys: (value: unknown) => void;
}) {
  const running = !!inf?.running;
  return (
    <div className="page">
      <div className="page-inner">
        <section className={`panel status-panel${running ? ' running' : ''}`}>
          <Lamp state={backendUp === false ? 'error' : running ? 'ready' : 'off'} />
          <div className="status-copy">
            <span className="eyebrow">Inference runtime</span>
            <strong>{!inf ? 'Runtime offline' : running ? inf.model ?? 'Model loaded' : 'Idle — no model running'}</strong>
            {inf && (
              <dl className="status-facts readout">
                <div><dt>Engine</dt><dd>{inf.engine}</dd></div>
                <div><dt>Context</dt><dd>{inf.context_size.toLocaleString()} tokens</dd></div>
                <div><dt>Server</dt><dd>{inf.base_url ?? '—'}</dd></div>
                <div><dt>Binary</dt><dd>{inf.binary_found ? 'Found' : 'Missing'}</dd></div>
              </dl>
            )}
          </div>
          <div className="status-actions">
            {!running && <Button icon="power" disabled={!modelId || !inf} onClick={onStart} data-tip={modelName ? `Start ${modelName}` : 'Select a model first'}>Start inference</Button>}
            {running && <Button variant="danger" icon="stop" onClick={() => inferenceStop().then(() => inferenceStatus().then(setInf)).catch((e) => notify('error', e.message))}>Stop</Button>}
            <Button variant="ghost" icon="refresh" onClick={() => { inferenceStatus().then(setInf).catch((e) => notify('error', e.message)); systemInfo().then(setSys).catch(() => {}); }}>Refresh</Button>
          </div>
        </section>

        {inf?.last_error && <Notice tone="error" title="The runtime reported an error">{inf.last_error}</Notice>}
        {inf && !inf.binary_found && (
          <Notice tone="caution" title="llama-server was not found">
            Download a llama.cpp release and place llama-server(.exe) with its DLLs in models/bin, put it on PATH, or set COMPANION_LLAMA_SERVER_BIN.
          </Notice>
        )}

        <DoctorCard notify={(kind, text) => notify(kind, text)} />
        <BenchmarkCard notify={(kind, text) => notify(kind, text)} />

        <div className="panel">
          <Section title="Hardware and runtime details" icon="cpu" collapsible defaultOpen={false}>
            <pre className="pre-block" style={{ maxHeight: 420 }}>{JSON.stringify(sys ?? { hint: 'backend offline' }, null, 2)}</pre>
          </Section>
        </div>
      </div>
    </div>
  );
}
