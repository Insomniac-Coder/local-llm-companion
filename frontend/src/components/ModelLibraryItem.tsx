import { useEffect, useId, useRef, useState } from 'react';
import { modelDetail, recheckTooling, type ModelDetail, type ModelMeta } from '../services/api';
import { Badge, Button, IconButton, Lamp } from '../ui/primitives';
import { toolSupportLabel, documentKindsNote } from '../services/toolSupport';
import { checkLabel, currentTooling, toolingBadge, toolingSummary } from '../services/tooling';
import RecommendCard from './RecommendCard';
import CalibrationCard from './CalibrationCard';
import OptimizeCard from './OptimizeCard';

type Notify = (kind: 'info' | 'success' | 'warning' | 'error', text: string) => void;
type Props = { model: ModelMeta; loadingModel: boolean; onLoad: () => void; onDelete: () => void; notify: Notify; onToolingChecked?: () => void };

/** What the model's tool check found, with Check again for the loaded model. */
export function ToolingDetails({ model, notify, onChecked }: { model: ModelMeta; notify: Notify; onChecked?: () => void }) {
  const [checking, setChecking] = useState(false);
  const profile = currentTooling(model);
  const shown = profile ?? model.tooling ?? null;
  const recheck = () => {
    setChecking(true);
    recheckTooling(model.id)
      .then((result) => { notify(result.tooling.can_write ? 'success' : 'warning', `Tool check: ${result.summary}`); onChecked?.(); })
      .catch((failure) => notify('error', failure instanceof Error ? failure.message : 'The tool check could not run.'))
      .finally(() => setChecking(false));
  };
  return <section className="model-tooling" aria-label={`Tool check for ${model.name}`}>
    <h3>Tool check</h3>
    {shown ? <>
      <p className="model-library-note">
        {toolingSummary(shown)}{' '}
        {profile ? `Checked ${new Date(shown.checked_at).toLocaleString()} in ${(shown.duration_ms / 1000).toFixed(1)} s.` : 'This check was made with an earlier check, runtime or chat template, so it runs again on the next load.'}
      </p>
      <ul className="model-tooling-checks">
        {shown.checks.map((check) => (
          <li key={check.name} className={check.passed ? 'passed' : 'failed'}>
            <span className="model-tooling-check-name">{checkLabel(check.name)}</span>
            <strong>{check.passed ? 'Passed' : 'Failed'}</strong>
            <code>{check.detail}</code>
          </li>
        ))}
      </ul>
    </> : <p className="model-library-note">Not checked yet. The first load checks how this model calls tools, which takes a few seconds.</p>}
    <div className="model-tooling-actions">
      <Button size="sm" variant="ghost" icon="refresh" disabled={!model.loaded || checking} onClick={recheck}>{checking ? 'Checking…' : 'Check again'}</Button>
      {!model.loaded && <span className="model-library-note">Load this model to check it again.</span>}
    </div>
  </section>;
}

export function ModelDetails({ detail, notify }: { detail: ModelDetail; notify: Notify }) {
  const { metadata, estimates } = detail;
  return <>
    <dl className="model-library-facts">
      <div><dt>Model file</dt><dd>{estimates.gguf_present ? estimates.file_gb != null ? `${estimates.file_gb} GB` : 'Size unavailable' : 'Missing'}</dd></div>
      <div><dt>GGUF</dt><dd>{estimates.gguf_present ? 'Present' : 'Missing'}</dd></div>
      <div><dt>Vision projector</dt><dd>{!metadata.vision ? 'Not required · text model' : estimates.projector_present ? 'Present' : 'Missing'}</dd></div>
      <div><dt>Architecture</dt><dd>{metadata.architecture || 'Unknown'}</dd></div>
      <div><dt>Estimated KV cache</dt><dd>~{estimates.kv_cache_gb} GB</dd></div>
      <div><dt>Estimated total memory</dt><dd>{estimates.total_need_gb != null ? `~${estimates.total_need_gb} GB` : 'Unavailable'}</dd></div>
    </dl>
    <div className="model-library-capabilities">
      <Badge tone={metadata.tool_calling ? 'info' : 'neutral'}>{toolSupportLabel(metadata)}</Badge>
      <Badge tone={metadata.vision ? 'info' : 'neutral'}>{metadata.vision ? 'Vision declared' : 'Text model'}</Badge>
      <Badge tone={metadata.supports_reasoning ? 'info' : 'neutral'}>{metadata.supports_reasoning ? 'Reasoning declared' : 'Reasoning unverified'}</Badge>
    </div>
    {documentKindsNote(metadata) && <p className="model-library-note">{documentKindsNote(metadata)}</p>}
    <p className="model-library-note">Memory estimates are heuristic and architecture-dependent, not measured usage or the current runtime cache allocation. Model capabilities are metadata declarations, not a compatibility test.</p>
    {estimates.compat_warnings.length > 0 && <ul className="model-library-warnings">{estimates.compat_warnings.map((warning, index) => <li key={index}>{warning}</li>)}</ul>}
    {detail.recommended && <p className="model-library-note">Estimate only · not applied: {detail.recommended.note}</p>}
    <div className="model-library-tools">
      <p className="model-library-note">Recommendations are not applied automatically. Choosing a context candidate saves it for the next model load.</p>
      <CalibrationCard modelId={metadata.id} notify={notify} />
      <RecommendCard modelId={metadata.id} notify={notify} />
      <OptimizeCard modelId={metadata.id} notify={notify} />
    </div>
  </>;
}

function ModelLibraryCard({ model, loadingModel, onLoad, onDelete, notify, onToolingChecked }: Props) {
  const [expanded, setExpanded] = useState(false);
  const [attempt, setAttempt] = useState(0);
  const [detail, setDetail] = useState<ModelDetail | null>(null);
  const [error, setError] = useState('');
  const [loading, setLoading] = useState(false);
  const request = useRef(0);
  const detailsButton = useRef<HTMLButtonElement>(null);
  const detailsId = useId();
  const headingId = useId();

  useEffect(() => {
    if (!expanded) return;
    const current = ++request.current;
    setLoading(true);
    setError('');
    modelDetail(model.id).then((result) => {
      if (current !== request.current) return;
      if (result.metadata.id !== model.id) throw new Error('The server returned details for a different model. Please retry.');
      setDetail(result);
    }).catch((failure) => {
      if (current === request.current) setError(failure instanceof Error ? failure.message : 'Model details are unavailable.');
    }).finally(() => { if (current === request.current) setLoading(false); });
    return () => { request.current++; };
  }, [expanded, attempt, model.id]);

  const close = () => {
    request.current++;
    setExpanded(false);
    setDetail(null);
    setError('');
    detailsButton.current?.focus();
  };

  const specs = [model.parameters && model.parameters !== 'unknown' ? model.parameters : null, model.quantization, `${model.context_length.toLocaleString()} context`].filter(Boolean);

  return <article className={`model-card${model.loaded ? ' loaded' : ''}${expanded ? ' expanded' : ''}`} aria-labelledby={headingId}>
    <div className="model-card-main">
      <Lamp state={model.loaded ? 'ready' : 'off'} />
      <div className="model-card-copy">
        <h2 id={headingId}>{model.name}</h2>
        <p className="readout">{specs.join(' · ')}</p>
      </div>
      <div className="model-library-capabilities">
        {model.loaded && <Badge tone="ok">Loaded</Badge>}
        {(model.tool_calling || model.tooling) && <Badge tone={toolingBadge(model).tone}>{toolingBadge(model).label}</Badge>}
        {model.vision && <Badge tone="info">Vision</Badge>}
      </div>
      <div className="model-library-actions">
        <Button type="button" size="sm" icon={model.loaded ? 'check' : 'power'} variant={model.loaded ? 'ghost' : 'secondary'} disabled={loadingModel || model.loaded} onClick={onLoad} aria-label={`Load ${model.name}`}>{model.loaded ? 'Loaded' : 'Load'}</Button>
        <button type="button" className="btn ghost sm" ref={detailsButton} aria-expanded={expanded} aria-controls={detailsId} aria-label={`${expanded ? 'Hide' : 'Show'} details for ${model.name}`} onClick={() => { if (expanded) close(); else { setLoading(true); setExpanded(true); } }}>
          <span className="btn-label">{expanded ? 'Hide details' : 'Details'}</span>
        </button>
        <IconButton icon="trash" label={`Delete ${model.name}`} size="md" tone="danger" tipSide="left" onClick={onDelete} />
      </div>
    </div>
    {/* Shown without expanding: a file the runtime will refuse should say so
        before Load is pressed, not in a log excerpt afterwards. */}
    {(model.load_issues?.length ?? 0) > 0 && (
      <ul className="model-library-warnings" aria-label={`Loading problems for ${model.name}`}>
        {model.load_issues!.map((issue, index) => <li key={index}>{issue}</li>)}
      </ul>
    )}
    <div className="model-library-detail" id={detailsId} hidden={!expanded} role="region" aria-label={`Details for ${model.name}`} aria-busy={expanded && loading}>
      {expanded && <>
        {loading && <p className="model-library-note" role="status">Loading details for {model.name}…</p>}
        {error && <div className="model-library-error" role="alert"><p>{error}</p><Button size="sm" onClick={() => { setLoading(true); setError(''); setAttempt((value) => value + 1); }}>Retry details</Button></div>}
        {!loading && !error && detail && <>
          <ToolingDetails model={model} notify={notify} onChecked={onToolingChecked} />
          <ModelDetails detail={detail} notify={notify} />
        </>}
      </>}
    </div>
  </article>;
}

/** Model identity owns the whole disclosure lifecycle, including in-flight reads. */
export default function ModelLibraryItem(props: Props) {
  return <ModelLibraryCard key={props.model.id} {...props} />;
}
