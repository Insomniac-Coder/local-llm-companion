import { useEffect, useMemo, useRef, useState } from 'react';
import { cacheBudget, getMetrics, getOverview, sessionAction } from '../services/api';
import ResourceChart from './ResourceChart';
import { formatReading, isReading, timeLabel, type ResourceSample } from './resourceTelemetry';
import { cacheDetails, type CacheDiagnostics } from './cacheDiagnostics';
import './resources.css';

const WINDOWS = [{ label: '5m', seconds: 300 }, { label: '15m', seconds: 900 }, { label: '30m', seconds: 1800 }, { label: '1h', seconds: 3600 }];
interface Alert { level: string; title: string; detail: string; suggestions?: string[] }
interface Metrics { latest: ResourceSample | null; samples: ResourceSample[]; alerts: Alert[] }
interface Session { id: string; title: string; mode: string; share: string; basis: string; cpu_pct?: number; gpu_pct?: number; vram_gb?: number }
interface Overview { sessions?: Session[]; inference?: { engine: string; model?: string; shared_weights_gb?: number }; alerts?: Alert[] }

export default function ResourcesPanel({ notify }: { notify?: (kind: 'info' | 'error', text: string) => void }) {
  const [range, setRange] = useState(WINDOWS[1]);
  const [metrics, setMetrics] = useState<Metrics | null>(null);
  const [overview, setOverview] = useState<Overview | null>(null);
  const [cache, setCache] = useState<CacheDiagnostics | null>(null);
  const [paused, setPaused] = useState(false);
  const [showIdle, setShowIdle] = useState(false);
  const [refresh, setRefresh] = useState(0);
  const [failed, setFailed] = useState(false);
  const [auxFailed, setAuxFailed] = useState(false);
  const [loading, setLoading] = useState(true);
  const [pending, setPending] = useState<string | null>(null);
  const lastOverview = useRef(0);
  const lastCache = useRef(0);

  useEffect(() => {
    if (paused) return;
    let disposed = false;
    let busy = false;
    let timer: ReturnType<typeof setTimeout>;
    let deadline: ReturnType<typeof setTimeout>;
    let controller: AbortController | null = null;
    const load = async () => {
      if (disposed || busy || document.hidden) return;
      busy = true;
      const now = Date.now();
      controller = new AbortController();
      deadline = setTimeout(() => controller?.abort(), 10_000);
      const readOverview = now - lastOverview.current > 15_000;
      const readCache = now - lastCache.current > 30_000;
      const responses = await Promise.allSettled([getMetrics(range.label, controller.signal), readOverview ? getOverview(controller.signal) : Promise.resolve(null), readCache ? cacheBudget(controller.signal) : Promise.resolve(null)]);
      clearTimeout(deadline);
      controller = null;
      busy = false;
      if (disposed) return;
      setLoading(false);
      const [metricResult, overviewResult, cacheResult] = responses;
      setFailed(metricResult.status === 'rejected');
      if (metricResult.status === 'fulfilled') setMetrics(metricResult.value as Metrics);
      if (overviewResult.status === 'fulfilled' && overviewResult.value) { setOverview(overviewResult.value); lastOverview.current = now; }
      if (cacheResult.status === 'fulfilled' && cacheResult.value) { setCache(cacheResult.value); lastCache.current = now; }
      if (readOverview || readCache) setAuxFailed(overviewResult.status === 'rejected' || cacheResult.status === 'rejected');
      timer = setTimeout(load, 5000);
    };
    const visible = () => { if (!document.hidden) { clearTimeout(timer); void load(); } };
    void load();
    document.addEventListener('visibilitychange', visible);
    return () => { disposed = true; clearTimeout(timer); clearTimeout(deadline); controller?.abort(); document.removeEventListener('visibilitychange', visible); };
  }, [range, paused, refresh]);

  const latest = metrics?.latest ?? null;
  // Older runtimes may downsample away the newest observation. Include it so
  // the chart endpoint and peak never contradict the current value.
  const samples = useMemo(() => {
    const history = metrics?.samples ?? [];
    return latest && (!history.length || latest.ts > history[history.length - 1].ts) ? [...history, latest] : history;
  }, [metrics, latest]);
  const sessions = overview?.sessions ?? [];
  const activeSessions = sessions.filter((session) => session.share === 'active');
  const visibleSessions = showIdle ? sessions : activeSessions;
  const alerts = metrics?.alerts ?? overview?.alerts ?? [];
  const inference = overview?.inference;
  const cacheInfo = cache ? cacheDetails(cache) : null;
  const stale = latest ? Date.now() / 1000 - latest.ts > 20 : false;
  const status = paused ? 'View paused' : failed ? 'Connection interrupted' : stale ? 'Waiting for fresh data' : latest ? 'Live telemetry' : loading ? 'Connecting' : 'Waiting for samples';
  const memoryMax = isReading(latest?.ram_total_gb) && latest.ram_total_gb > 0 ? latest.ram_total_gb : Math.max(1, ...samples.map((sample) => sample.ram_total_gb ?? 0));
  const vramMax = isReading(latest?.vram_total_gb) && latest.vram_total_gb > 0 ? latest.vram_total_gb : Math.max(1, ...samples.map((sample) => sample.vram_total_gb ?? 0));

  const act = async (session: Session, action: 'stop' | 'reduce') => {
    setPending(session.id);
    try {
      const response = await sessionAction(session.id, action);
      const detail = Array.isArray(response.detail) ? response.detail.join('. ') : response.detail;
      notify?.('info', detail || (action === 'stop' ? 'Session stopped.' : 'Context compacted.'));
      lastOverview.current = 0;
      setRefresh((value) => value + 1);
    } catch (error) { notify?.('error', error instanceof Error ? error.message : 'Session action failed.'); }
    finally { setPending(null); }
  };

  return (
    <div className="resource-dashboard">
      <header className="rd-heading">
        <div><h1>Resources</h1><p>Your machine, in real time.</p></div>
        <div className="rd-controls"><div className="rd-ranges" role="group" aria-label="Resource history range">{WINDOWS.map((item) => <button key={item.label} aria-pressed={range.label === item.label} onClick={() => setRange(item)}>{item.label}</button>)}</div><button className="rd-pause" onClick={() => setPaused((value) => !value)} aria-pressed={paused} title="Pause this view only. Model execution is not affected.">{paused ? 'Resume live view' : 'Pause view'}</button></div>
      </header>
      <div className="rd-statusline"><span className={`rd-status ${!paused && !failed && !stale && latest ? 'is-live' : ''}`}><i />{status}</span><span>{latest ? `Last sample ${timeLabel(latest.ts)}` : 'System-wide readings'}</span><span className="rd-host-note">Whole machine · not per-process</span></div>
      {failed && <div className="rd-notice" role="alert"><span>{latest ? 'Showing the last available readings. The local runtime is not responding.' : 'Cannot reach the local runtime. Start the backend to see resource use.'}</span><button onClick={() => { setPaused(false); setRefresh((value) => value + 1); }}>Retry</button></div>}

      <section className="rd-charts" aria-label="Live resource history">
        <ResourceChart samples={samples} latest={latest} field="cpu_pct" label="Processor" detail="System CPU utilization" maximum={100} unit="%" tone="cpu" seconds={range.seconds} />
        <ResourceChart samples={samples} latest={latest} field="ram_used_gb" label="Memory" detail={isReading(latest?.ram_total_gb) ? `of ${formatReading(latest.ram_total_gb, 1)} GB system RAM` : 'System RAM'} maximum={memoryMax} unit="GB" tone="ram" seconds={range.seconds} />
        <ResourceChart samples={samples} latest={latest} field="gpu_pct" label="Graphics processor" detail="Reported by the GPU driver" maximum={100} unit="%" tone="gpu" seconds={range.seconds} />
        <ResourceChart samples={samples} latest={latest} field="vram_used_gb" label="Graphics memory" detail={isReading(latest?.vram_total_gb) ? `of ${formatReading(latest.vram_total_gb, 1)} GB dedicated VRAM` : 'Dedicated GPU memory'} maximum={vramMax} unit="GB" tone="vram" seconds={range.seconds} />
      </section>

      <section className="rd-runtime" aria-label="Inference runtime"><div><span className="rd-runtime-icon" aria-hidden="true">⌁</span><div><h2>{inference?.model || 'No model running'}</h2><p>{inference?.model ? `${inference.engine} · shared across sessions` : overview ? 'Load a model to start local inference.' : 'Runtime details are loading.'}</p></div></div><dl>{isReading(inference?.shared_weights_gb) && <div><dt>Model on disk</dt><dd>{formatReading(inference.shared_weights_gb, 1)} <span>GB</span></dd></div>}{isReading(latest?.gpu_temp_c) && <div><dt>GPU temperature</dt><dd>{formatReading(latest.gpu_temp_c)}<span>°C</span></dd></div>}{isReading(latest?.gpu_power_w) && <div><dt>GPU power</dt><dd>{formatReading(latest.gpu_power_w)} <span>W</span></dd></div>}</dl></section>

      {alerts.length > 0 && <section className="rd-alerts" aria-label="Resource alerts">{alerts.map((alert, index) => <article key={`${alert.title}-${index}`}><span aria-hidden="true">!</span><div><h2>{alert.title}</h2><p>{alert.detail}</p>{alert.suggestions?.map((suggestion) => <small key={suggestion}>{suggestion}</small>)}</div></article>)}</section>}

      <section className="rd-section"><header><div><h2>Sessions <span className="rd-count">{activeSessions.length} active</span></h2><p>Readings above belong to the whole machine; individual session use is not measured.</p></div><label className="rd-idle-toggle"><input type="checkbox" checked={showIdle} onChange={(event) => setShowIdle(event.target.checked)} />Show idle</label></header>
        {!visibleSessions.length && <div className="rd-empty"><span aria-hidden="true">◌</span><div><strong>{showIdle ? 'No sessions yet' : 'No active sessions'}</strong><p>{showIdle ? 'Start a chat or coding task to create one.' : 'Running chats and coding tasks will appear here.'}</p></div></div>}
        {visibleSessions.map((session) => <article className="rd-session" key={session.id}><span className="rd-session-icon" aria-hidden="true">{session.mode === 'code' ? '</>' : '↳'}</span><div className="rd-session-copy"><h3>{session.title}</h3><p><span className={session.share === 'active' ? 'rd-session-active' : ''}>{session.share === 'active' ? 'Running' : 'Idle'}</span><span>{session.mode === 'code' ? 'Code session' : 'Chat session'}</span></p></div><div className="rd-session-actions"><button disabled={pending !== null || !inference?.model || session.share === 'active'} title={session.share === 'active' ? 'Stop the running task before compacting context.' : 'Summarize older context to reduce memory pressure.'} onClick={() => void act(session, 'reduce')}>{pending === session.id ? 'Working…' : 'Compact context'}</button>{session.share === 'active' && <button className="rd-stop" disabled={pending !== null} onClick={() => void act(session, 'stop')}>Stop</button>}</div></article>)}
      </section>

      <details className="rd-section rd-cache" open><summary><span>Runtime memory &amp; history</span><span>{cacheInfo?.capacity ?? 'Runtime details'}</span></summary>{cache && cacheInfo ? <><dl><div><dt>Chart sample history</dt><dd>{cache.sampler?.samples?.toLocaleString() ?? '—'} <span>samples</span></dd><p>{formatReading(isReading(cache.sampler?.bytes_est) ? cache.sampler.bytes_est / 1024 : null, 1)} KB estimated{cache.sampler?.cadence_secs ? ` · every ${cache.sampler.cadence_secs}s` : ''}</p></div><div><dt>Model cache usage</dt><dd>{cacheInfo.runtimeSize}</dd><p>{cacheInfo.runtimeNote}</p></div><div><dt>Saved history</dt><dd>{cacheInfo.history}</dd><p>{cacheInfo.historyNote}</p></div></dl><p className="rd-cache-note">{cacheInfo.switchNote}</p></> : <p className="rd-cache-note">{loading ? 'Loading runtime details…' : 'Runtime details are unavailable.'}</p>}</details>
      {auxFailed && <p className="rd-footnote" role="status">Some session or cache details could not be refreshed. Retrying automatically.</p>}
      <p className="rd-footnote">Charts refresh every 5 seconds while this view is visible. Missing sensors are shown as unavailable, never as zero.</p>
    </div>
  );
}
