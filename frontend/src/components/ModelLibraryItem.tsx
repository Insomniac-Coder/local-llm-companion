import { useEffect, useId, useRef, useState } from 'react';
import { modelDetail, type ModelDetail, type ModelMeta } from '../services/api';
import { Badge } from '../ui/primitives';
import RecommendCard from './RecommendCard';
import OptimizeCard from './OptimizeCard';
import './modelLibrary.css';

type Notify = (kind: 'info' | 'success' | 'warning' | 'error', text: string) => void;
type Props = { model: ModelMeta; loadingModel: boolean; onLoad: () => void; onDelete: () => void; notify: Notify };

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
    <p className="model-library-note">Memory estimates are heuristic and architecture-dependent, not measured usage or the current runtime cache allocation. Model capabilities are metadata declarations, not a compatibility test.</p>
    <div className="model-library-capabilities">
      <Badge tone={metadata.tool_calling ? 'info' : 'neutral'}>{metadata.tool_calling ? 'Tools declared' : 'Tools unverified'}</Badge>
      <Badge tone={metadata.vision ? 'info' : 'neutral'}>{metadata.vision ? 'Vision declared' : 'Text model'}</Badge>
      <Badge tone={metadata.supports_reasoning ? 'info' : 'neutral'}>{metadata.supports_reasoning ? 'Reasoning declared' : 'Reasoning unverified'}</Badge>
    </div>
    {estimates.compat_warnings.length > 0 && <ul className="model-library-warnings">{estimates.compat_warnings.map((warning, index) => <li key={index}>{warning}</li>)}</ul>}
    {detail.recommended && <p className="model-library-note">Estimate only · not applied: {detail.recommended.note}</p>}
    <div className="model-library-tools">
      <p className="model-library-note">Recommendations are not applied automatically. Choosing a context candidate saves it for the next model load.</p>
      <RecommendCard modelId={metadata.id} notify={notify} />
      <OptimizeCard modelId={metadata.id} notify={notify} />
    </div>
  </>;
}

function ModelLibraryCard({ model, loadingModel, onLoad, onDelete, notify }: Props) {
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

  return <article className={`card model-library-card${model.loaded ? ' loaded' : ''}`} aria-labelledby={headingId}>
    <header className="model-library-heading"><h2 id={headingId}>{model.name}</h2><Badge tone={model.loaded ? 'ok' : 'neutral'}>{model.loaded ? 'Loaded' : 'Idle'}</Badge></header>
    <p className="model-library-meta">{model.parameters} · {model.quantization} · {model.context_length.toLocaleString()} context</p>
    <div className="model-library-capabilities"><Badge tone={model.tool_calling ? 'info' : 'neutral'}>{model.tool_calling ? 'Tools declared' : 'Tools unverified'}</Badge><Badge tone={model.vision ? 'info' : 'neutral'}>{model.vision ? 'Vision declared' : 'Text model'}</Badge></div>
    <div className="model-library-actions">
      <button type="button" disabled={loadingModel || model.loaded} onClick={onLoad} aria-label={`Load ${model.name}`}>{model.loaded ? 'Loaded' : 'Load model'}</button>
      <button type="button" ref={detailsButton} aria-expanded={expanded} aria-controls={detailsId} aria-label={`${expanded ? 'Hide' : 'Show'} details for ${model.name}`} onClick={() => { if (expanded) close(); else { setLoading(true); setExpanded(true); } }}>{expanded ? 'Hide details' : 'Details'}</button>
      <button type="button" className="model-library-delete" onClick={onDelete} aria-label={`Delete ${model.name}`}>Delete</button>
    </div>
    <div className="model-library-detail" id={detailsId} hidden={!expanded} role="region" aria-label={`Details for ${model.name}`} aria-busy={expanded && loading}>
      {expanded && <>
        <div className="model-library-detail-heading"><h3>Model details</h3><button type="button" onClick={close} aria-label={`Close details for ${model.name}`}>Close</button></div>
        {loading && <p className="model-library-note" role="status">Loading details for {model.name}…</p>}
        {error && <div className="model-library-error" role="alert"><p>{error}</p><button type="button" onClick={() => { setLoading(true); setError(''); setAttempt((value) => value + 1); }}>Retry details</button></div>}
        {!loading && !error && detail && <ModelDetails detail={detail} notify={notify} />}
      </>}
    </div>
  </article>;
}

/** Model identity owns the whole disclosure lifecycle, including in-flight reads. */
export default function ModelLibraryItem(props: Props) {
  return <ModelLibraryCard key={props.model.id} {...props} />;
}
