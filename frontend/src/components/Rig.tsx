import { loadErrorSummary } from '../services/loadError';
import { useEffect, useState } from 'react';
import { cancelLoad, getLoadProgress, getMetrics, type InferenceStatus, type LoadProgress, type ModelMeta } from '../services/api';
import { Button, IconButton, Lamp, Meter, PopDivider, PopItem, PopLabel, Popover } from '../ui/primitives';
import { Icon } from '../ui/Icon';
import { isReading, type ResourceSample } from './resourceTelemetry';
import StartScripts from './StartScripts';

export type MachineActivity = 'idle' | 'generating' | 'agent' | 'waiting';

type Props = {
  models: ModelMeta[];
  modelId: string;
  inf: InferenceStatus | null;
  backendUp: boolean | null;
  loadingModel: boolean;
  activity: MachineActivity;
  liveTps: number | null;
  lastTps: number | null;
  /** What the live work is doing when there is no speed to show yet. */
  phaseLabel?: string;
  collapsed: boolean;
  onSelect: (id: string) => void;
  onLoad: (id: string) => void;
  onUnload: () => void;
  onReload: () => void;
  onOpenModels: () => void;
  onOpenResources: () => void;
  notify: (kind: 'info' | 'error', text: string) => void;
};

export type RigState = 'off' | 'ready' | 'caution' | 'live' | 'error';

/** The single machine state the whole UI agrees on (lamp colour, labels). */
export function machineState(backendUp: boolean | null, loading: boolean, activity: MachineActivity, running: boolean): { state: RigState; label: string } {
  if (backendUp === false) return { state: 'error', label: 'Runtime offline' };
  if (loading) return { state: 'caution', label: 'Loading model' };
  if (activity === 'waiting') return { state: 'caution', label: 'Needs your approval' };
  if (activity === 'generating') return { state: 'live', label: 'Generating' };
  if (activity === 'agent') return { state: 'live', label: 'Agent working' };
  if (running) return { state: 'ready', label: 'Ready' };
  return { state: 'off', label: 'No model loaded' };
}

const fmtContext = (tokens: number | null | undefined) => !tokens ? '' : tokens >= 1000 ? `${Math.round(tokens / 102.4) / 10}K` : `${tokens}`;
const gb = (value: number | null | undefined, digits = 1) => typeof value === 'number' && Number.isFinite(value) ? value.toFixed(digits) : '—';
const shortGpu = (name: string | null) => name?.replace(/^NVIDIA\s+/i, '').replace(/^GeForce\s+/i, '') ?? null;

function spec(model: ModelMeta | undefined, context: number | null) {
  if (!model) return '';
  return [model.parameters && model.parameters !== 'unknown' ? model.parameters : null, model.quantization, context ? `${fmtContext(context)} context` : null].filter(Boolean).join(' · ');
}

/** Latest whole-machine reading plus the GPU name, polled while the page is visible. */
export function useMachineSample() {
  const [sample, setSample] = useState<ResourceSample | null>(null);
  const [gpuName, setGpuName] = useState<string | null>(null);
  useEffect(() => {
    let disposed = false;
    let timer: ReturnType<typeof setTimeout>;
    const read = async () => {
      if (disposed) return;
      if (!document.hidden) {
        try {
          const metrics = await getMetrics('5m');
          if (!disposed) {
            setSample(metrics.latest as ResourceSample | null);
            if (metrics.gpu_name) setGpuName(metrics.gpu_name);
          }
        } catch { /* the lamp already reports an offline runtime */ }
      }
      timer = setTimeout(read, 4000);
    };
    void read();
    return () => { disposed = true; clearTimeout(timer); };
  }, []);
  return { sample, gpuName };
}

function useLoadProgress(active: boolean) {
  const [progress, setProgress] = useState<LoadProgress | null>(null);
  useEffect(() => {
    if (!active) { setProgress(null); return; }
    let stopped = false;
    const tick = () => getLoadProgress().then((value) => { if (!stopped) setProgress(value); }).catch(() => {});
    tick();
    const timer = setInterval(tick, 1200);
    return () => { stopped = true; clearInterval(timer); };
  }, [active]);
  return progress;
}

export default function Rig(props: Props) {
  const { models, modelId, inf, backendUp, loadingModel, activity, liveTps, lastTps, collapsed } = props;
  const [menuOpen, setMenuOpen] = useState(false);
  // Polled here rather than in the app root, so telemetry ticks re-render only this panel.
  const { sample, gpuName } = useMachineSample();
  const progress = useLoadProgress(loadingModel);
  const loaded = models.find((model) => model.loaded && inf?.running);
  const selected = models.find((model) => model.id === modelId);
  const shown = loaded ?? selected;
  const { state, label } = machineState(backendUp, loadingModel, activity, !!loaded);
  const live = state === 'live';
  const context = loaded ? inf?.context_size ?? null : shown?.context_length ?? null;
  const vramKnown = typeof sample?.vram_total_gb === 'number' && sample.vram_total_gb > 0;

  const menu = (
    <Popover open={menuOpen} onClose={() => setMenuOpen(false)} label="Models" side={collapsed ? 'right' : 'top'} align="start">
      <PopLabel>{gpuName ? shortGpu(gpuName) : 'Models on this PC'}</PopLabel>
      {models.length === 0 && <div className="attention-note">No models found. Add a GGUF file to the models folder, then scan.</div>}
      {models.map((model) => (
        <button
          key={model.id}
          type="button"
          role="menuitemradio"
          aria-checked={model.id === modelId}
          className="rig-menu-model"
          onClick={() => { props.onSelect(model.id); if (!model.loaded) setMenuOpen(false); }}
        >
          <span className="rig-menu-check">{model.id === modelId && <Icon name="check" size={14} />}</span>
          <strong>{model.name}</strong>
          <small>{spec(model, model.context_length)}</small>
          {model.loaded && inf?.running && <span className="badge ok">Loaded</span>}
        </button>
      ))}
      <PopDivider />
      {selected && !selected.loaded && (
        <PopItem icon="power" disabled={loadingModel || backendUp === false} onClick={() => { setMenuOpen(false); props.onLoad(selected.id); }}>
          Load {selected.name}
        </PopItem>
      )}
      {loaded && <PopItem icon="refresh" disabled={loadingModel} onClick={() => { setMenuOpen(false); props.onReload(); }}>Reload model</PopItem>}
      {loaded && <PopItem icon="eject" disabled={loadingModel} onClick={() => { setMenuOpen(false); props.onUnload(); }}>Eject model</PopItem>}
      <PopItem icon="layers" onClick={() => { setMenuOpen(false); props.onOpenModels(); }}>Manage models</PopItem>
      <PopItem icon="activity" onClick={() => { setMenuOpen(false); props.onOpenResources(); }}>Resource monitor</PopItem>
    </Popover>
  );

  if (collapsed) {
    return (
      <div className="rig-rail">
        <button type="button" className="rig-rail-btn" aria-label={`${label}${shown ? ` · ${shown.name}` : ''}. Open model menu.`} data-tip={shown ? `${label} · ${shown.name}` : label} data-tip-side="right" aria-expanded={menuOpen} onClick={() => setMenuOpen((open) => !open)}>
          <Lamp state={state} pulse={live || state === 'caution'} />
          {/* The memory the model lives in: graphics memory on a GPU machine, system memory otherwise. */}
          {vramKnown
            ? <Meter value={sample?.vram_used_gb} max={sample?.vram_total_gb} cells={6} label="Graphics memory" />
            : <Meter value={sample?.ram_used_gb} max={sample?.ram_total_gb} cells={6} label="System memory" />}
        </button>
        {menu}
      </div>
    );
  }

  const stage = progress && progress.stage !== 'idle' && progress.stage !== 'ready' ? progress : null;

  return (
    <div className="rig">
      <div className={`rig-panel ${state}`} aria-label="Machine status" role="group">
        <div className="rig-head">
          <div className="rig-state">
            <Lamp state={state} pulse={live || state === 'caution'} />
            <span className="eyebrow" role="status" aria-live="polite">{label}</span>
          </div>
          <IconButton icon="chevronsUpDown" label="Switch model" size="sm" tipSide="top" aria-haspopup="menu" aria-expanded={menuOpen} onClick={() => setMenuOpen((open) => !open)} />
        </div>
        {menu}

        <button type="button" className="rig-model" onClick={() => setMenuOpen((open) => !open)} title={shown?.name}>
          <span>{shown?.name ?? (backendUp === false ? 'Models unavailable' : models.length ? 'Choose a model' : 'No models installed')}</span>
        </button>
        {shown && <div className="rig-spec readout">{spec(shown, context)}</div>}

        {backendUp === false && <p className="rig-detail error">Start Companion again with <StartScripts /> to reconnect.</p>}

        {stage && (
          <>
            {stage.stage === 'error'
              ? <p className="rig-detail error" title={stage.detail}>{loadErrorSummary(stage.detail)}</p>
              : <p className="rig-detail">{stage.detail || `${stage.stage}…`}</p>}
            {stage.stage !== 'error' && <div className="rig-progress" aria-hidden="true"><i /></div>}
          </>
        )}
        {!stage && !loadingModel && inf?.last_error && !loaded && backendUp !== false && <p className="rig-detail error" title={inf.last_error}>{loadErrorSummary(inf.last_error)}</p>}

        {sample && backendUp !== false && (
          <div className="rig-meters">
            {/* Processor and system memory first: they exist on every machine
                and on a CPU-only one they are the whole engine. Graphics rows
                follow when a GPU reports. Nothing here is hidden for space. */}
            <span className="eyebrow">CPU</span>
            <Meter value={sample.cpu_pct} max={100} label="Processor load" warn={false} />
            <span className="readout">{isReading(sample.cpu_pct) ? `${Math.round(sample.cpu_pct)}%` : '—'}</span>
            <span className="eyebrow">RAM</span>
            <Meter value={sample.ram_used_gb} max={sample.ram_total_gb} label="System memory in use" />
            <span className="readout">{gb(sample.ram_used_gb, 0)}/{gb(sample.ram_total_gb, 0)} GB</span>
            {typeof sample.gpu_pct === 'number' && <>
              <span className="eyebrow" title={gpuName ?? 'Graphics processor'}>GPU</span>
              <Meter value={sample.gpu_pct} max={100} label="Graphics processor load" warn={false} />
              <span className="readout">{Math.round(sample.gpu_pct)}%{typeof sample.gpu_temp_c === 'number' ? ` · ${Math.round(sample.gpu_temp_c)}°` : ''}</span>
            </>}
            {vramKnown && <>
              <span className="eyebrow" title={gpuName ?? 'Dedicated graphics memory'}>VRAM</span>
              <Meter value={sample.vram_used_gb} max={sample.vram_total_gb} label="Graphics memory in use" />
              <span className={`readout${(sample.vram_used_gb ?? 0) / (sample.vram_total_gb ?? 1) >= 0.9 ? ' hot' : ''}`}>{gb(sample.vram_used_gb)}/{gb(sample.vram_total_gb)} GB</span>
            </>}
          </div>
        )}

        <div className="rig-foot">
          {loadingModel ? (
            <>
              <div className="rig-speed"><span>Preparing the runtime</span></div>
              {(stage?.stage === 'validating' || stage?.stage === 'loading') && (
                <Button size="sm" variant="ghost" onClick={() => cancelLoad().then(() => props.notify('info', 'Model load cancelled.')).catch((error) => props.notify('error', error.message))}>Cancel</Button>
              )}
            </>
          ) : loaded ? (
            <>
              <div className="rig-speed">
                {activity === 'waiting' ? <span>Waiting for your approval</span>
                  : live && liveTps != null ? <><strong>{liveTps.toFixed(1)}</strong><span>tok/s now</span></>
                    : !live && lastTps != null ? <><strong>{lastTps.toFixed(1)}</strong><span>tok/s last reply</span></>
                      : <span>{live ? props.phaseLabel ?? 'Working…' : 'Waiting for a message'}</span>}
              </div>
              <IconButton icon="eject" label="Eject model" size="sm" tipSide="top" disabled={live} onClick={props.onUnload} />
            </>
          ) : (
            <>
              <div className="rig-speed"><span>{backendUp === false ? 'Offline' : !selected ? 'Pick a model first' : vramKnown ? 'Loads into GPU memory' : 'Loads into memory'}</span></div>
              <Button size="sm" variant="primary" icon="power" disabled={!selected || backendUp === false} onClick={() => selected && props.onLoad(selected.id)}>Load</Button>
            </>
          )}
        </div>
      </div>
    </div>
  );
}
