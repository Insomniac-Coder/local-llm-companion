import { useEffect, useMemo, useRef, useState } from 'react';
import { cacheBudget, getMetrics, getOverview, sessionAction } from '../services/api';
import ResourceChart from './ResourceChart';
import { formatReading, isReading, timeLabel, type ResourceSample } from './resourceTelemetry';
import { cacheDetails, type CacheDiagnostics } from './cacheDiagnostics';
import { Button, Lamp, Notice, Section } from '../ui/primitives';
import { Icon } from '../ui/Icon';

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
  const live = !paused && !failed && !stale && !!latest;
  const status = paused ? 'View paused' : failed ? 'Connection interrupted' : stale ? 'Waiting for fresh data' : latest ? 'Live' : loading ? 'Connecting' : 'Waiting for samples';
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
    <div className="page">
      <div className="page-inner wide">
        <div className="rd-toolbar">
          <span className="rd-status"><Lamp state={live ? 'ready' : failed ? 'error' : 'caution'} pulse={live} />{status}</span>
          <span className="rd-statusline">
            <span className="readout">{latest ? `Last sample ${timeLabel(latest.ts)}` : 'System-wide readings'}</span>
            <span>Whole machine, not per process</span>
          </span>
          <div className="rd-controls">
            <div className="rd-ranges" role="group" aria-label="Resource history range">{WINDOWS.map((item) => <button type="button" key={item.label} aria-pressed={range.label === item.label} onClick={() => setRange(item)}>{item.label}</button>)}</div>
            <Button variant="ghost" size="sm" icon={paused ? 'play' : 'pause'} onClick={() => setPaused((value) => !value)} aria-pressed={paused} title="Pause this view only. Model execution is not affected.">{paused ? 'Resume' : 'Pause'}</Button>
          </div>
        </div>

        {failed && (
          <Notice tone="error" title={latest ? 'Showing the last readings' : 'Can’t reach the local runtime'} actions={<Button size="sm" onClick={() => { setPaused(false); setRefresh((value) => value + 1); }}>Retry</Button>}>
            {latest ? 'The local runtime is not responding right now.' : 'Start the backend to see resource use.'}
          </Notice>
        )}

        <section className="rd-charts" aria-label="Live resource history">
          <ResourceChart samples={samples} latest={latest} field="cpu_pct" label="Processor" detail="System CPU utilization" maximum={100} unit="%" tone="cpu" seconds={range.seconds} />
          <ResourceChart samples={samples} latest={latest} field="ram_used_gb" label="Memory" detail={isReading(latest?.ram_total_gb) ? `of ${formatReading(latest.ram_total_gb, 1)} GB system RAM` : 'System RAM'} maximum={memoryMax} unit="GB" tone="ram" seconds={range.seconds} />
          <ResourceChart samples={samples} latest={latest} field="gpu_pct" label="Graphics processor" detail="Reported by the GPU driver" maximum={100} unit="%" tone="gpu" seconds={range.seconds} />
          <ResourceChart samples={samples} latest={latest} field="vram_used_gb" label="Graphics memory" detail={isReading(latest?.vram_total_gb) ? `of ${formatReading(latest.vram_total_gb, 1)} GB dedicated VRAM` : 'Dedicated GPU memory'} maximum={vramMax} unit="GB" tone="vram" seconds={range.seconds} />
        </section>

        <section className="panel rd-runtime" aria-label="Inference runtime">
          <div>
            <span className="rd-runtime-icon" aria-hidden="true"><Icon name="layers" size={18} /></span>
            <div><h2>{inference?.model || 'No model running'}</h2><p>{inference?.model ? `${inference.engine} · shared across sessions` : overview ? 'Load a model to start local inference.' : 'Runtime details are loading.'}</p></div>
          </div>
          <dl>
            {isReading(inference?.shared_weights_gb) && <div><dt>Model on disk</dt><dd>{formatReading(inference.shared_weights_gb, 1)} <span>GB</span></dd></div>}
            {isReading(latest?.gpu_temp_c) && <div><dt>GPU temperature</dt><dd>{formatReading(latest.gpu_temp_c)}<span>°C</span></dd></div>}
            {isReading(latest?.gpu_power_w) && <div><dt>GPU power</dt><dd>{formatReading(latest.gpu_power_w)} <span>W</span></dd></div>}
          </dl>
        </section>

        {alerts.length > 0 && (
          <section className="rd-alerts" aria-label="Resource alerts">
            {alerts.map((alert, index) => (
              <Notice key={`${alert.title}-${index}`} tone="caution" title={alert.title}>
                {alert.detail}{alert.suggestions?.length ? ` Try: ${alert.suggestions.join(' · ')}.` : ''}
              </Notice>
            ))}
          </section>
        )}

        <section className="panel rd-section">
          <header>
            <div><h2>Sessions <span className="rd-count">{activeSessions.length} active</span></h2><p>Readings above belong to the whole machine; individual session use is not measured.</p></div>
            <label className="rd-idle-toggle"><input type="checkbox" checked={showIdle} onChange={(event) => setShowIdle(event.target.checked)} />Show idle</label>
          </header>
          {!visibleSessions.length && <p className="help">{showIdle ? 'No sessions yet. Start a chat or coding task to create one.' : 'No active sessions. Running chats and coding tasks appear here.'}</p>}
          {visibleSessions.map((session) => (
            <article className="rd-session" key={session.id}>
              <span className="rd-session-icon" aria-hidden="true"><Icon name={session.mode === 'code' ? 'code' : 'chat'} size={15} /></span>
              <div className="rd-session-copy">
                <h3>{session.title}</h3>
                <p><span style={{ display: 'inline-flex', alignItems: 'center', gap: 6 }}><Lamp state={session.share === 'active' ? 'live' : 'off'} pulse={session.share === 'active'} />{session.share === 'active' ? 'Running' : 'Idle'}</span><span>{session.mode === 'code' ? 'Code session' : 'Chat session'}</span></p>
              </div>
              <div className="rd-session-actions">
                <Button size="sm" variant="ghost" disabled={pending !== null || !inference?.model || session.share === 'active'} title={session.share === 'active' ? 'Stop the running task before compacting context.' : 'Summarize older context to reduce memory pressure.'} loading={pending === session.id} onClick={() => void act(session, 'reduce')}>Compact context</Button>
                {session.share === 'active' && <Button size="sm" variant="danger" icon="stop" disabled={pending !== null} onClick={() => void act(session, 'stop')}>Stop</Button>}
              </div>
            </article>
          ))}
        </section>

        <div className="panel rd-cache">
          <Section title="Runtime memory & history" icon="memory" meta={cacheInfo?.capacity} collapsible defaultOpen>
            {cache && cacheInfo ? (
              <>
                <dl>
                  <div><dt>Chart sample history</dt><dd>{cache.sampler?.samples?.toLocaleString() ?? '—'} <span>samples</span></dd><p>{formatReading(isReading(cache.sampler?.bytes_est) ? cache.sampler.bytes_est / 1024 : null, 1)} KB estimated{cache.sampler?.cadence_secs ? ` · every ${cache.sampler.cadence_secs}s` : ''}</p></div>
                  <div><dt>Model cache usage</dt><dd>{cacheInfo.runtimeSize}</dd><p>{cacheInfo.runtimeNote}</p></div>
                  <div><dt>Saved history</dt><dd>{cacheInfo.history}</dd><p>{cacheInfo.historyNote}</p></div>
                </dl>
                <p className="help">{cacheInfo.switchNote}</p>
              </>
            ) : <p className="help">{loading ? 'Loading runtime details…' : 'Runtime details are unavailable.'}</p>}
          </Section>
        </div>
        {auxFailed && <p className="rd-footnote" role="status">Some session or cache details could not be refreshed. Retrying automatically.</p>}
        <p className="rd-footnote">Charts refresh every 5 seconds while this view is visible. Missing sensors are shown as unavailable, never as zero. The red point marks the latest reading.</p>
      </div>
    </div>
  );
}
