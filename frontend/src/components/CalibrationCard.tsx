import { useEffect, useRef, useState } from 'react';
import {
  calibrateModel, getCalibration, placementLabel, PROFILE_ORDER, profileSummary, profileTitle, staleReason, summarizeMicroBatches,
  type CalibrationStatus,
} from '../services/calibration';
import { Badge, Button } from '../ui/primitives';

type Notify = (kind: 'info' | 'success' | 'warning' | 'error', text: string) => void;

/** Measured profiles for one model on this machine, and the action that
 * measures them. Unlike the suggestions card these are benchmark results, and
 * the profile selected in Settings > Performance is applied at the next load. */
export default function CalibrationCard({ modelId, notify }: { modelId: string; notify: Notify }) {
  const [status, setStatus] = useState<CalibrationStatus | null>(null);
  const [stage, setStage] = useState('');
  const [running, setRunning] = useState(false);
  const [showMeasurements, setShowMeasurements] = useState(false);
  const abort = useRef<AbortController | null>(null);

  useEffect(() => {
    let current = true;
    getCalibration(modelId).then((next) => { if (current) setStatus(next); }).catch(() => {});
    return () => { current = false; abort.current?.abort(); };
  }, [modelId]);

  const run = async () => {
    abort.current = new AbortController();
    setRunning(true);
    setStage('Starting…');
    try {
      await calibrateModel(modelId, setStage, abort.current.signal);
      setStatus(await getCalibration(modelId));
      notify('success', 'Calibration saved. Choose a profile in Settings > Performance; it applies at the next model load.');
    } catch (error) {
      if ((error as Error).name !== 'AbortError') notify('error', (error as Error).message);
    } finally {
      setRunning(false);
      setStage('');
    }
  };

  const calibration = status?.calibration ?? null;
  const stale = staleReason(status);
  const microBatches = summarizeMicroBatches(calibration?.micro_batches);
  const signed = (percent: number | null) => (percent === null ? '' : ` (${percent > 0 ? '+' : ''}${percent}% vs 512)`);
  return (
    <div className="tool-card calibration-card">
      <header>
        <strong>Performance profiles</strong>
        <Badge tone={calibration ? (stale ? 'warn' : 'ok') : 'neutral'}>{calibration ? (stale ? 'Out of date' : 'Measured') : 'Not calibrated'}</Badge>
      </header>
      <p>
        Measures this model on this machine at several thread settings and keeps three profiles: Fastest, Balanced and Light.
        Calibrating unloads the current model first, so only one model is ever in memory, and takes a minute or two. With a GPU it also
        measures micro-batch sizes in two passes with a 2-minute pause between them, which adds several minutes.
      </p>
      <div className="controls">
        <Button size="sm" loading={running} onClick={() => void run()}>{calibration ? 'Calibrate again' : 'Calibrate'}</Button>
        {running && <span className="calibration-stage" role="status">{stage}</span>}
      </div>
      {calibration && (
        <>
          {stale && <p className="calibration-warning">{stale}</p>}
          {calibration.regression && (
            <p className="calibration-warning" role="alert">
              Slower than the previous comparable calibration: {calibration.regression.current_tps.toFixed(1)} tok/s vs {calibration.regression.previous_tps.toFixed(1)} tok/s ({calibration.regression.change_percent}%).
            </p>
          )}
          <ul className="calibration-profiles">
            {PROFILE_ORDER.map((name) => calibration.profiles.find((profile) => profile.name === name)).filter(Boolean).map((profile) => (
              <li key={profile!.name}>
                <strong>{profileTitle(profile!.name)}</strong>
                <span className="readout">{profileSummary(profile!)}</span>
                <span className="calibration-note">{profile!.description}</span>
              </li>
            ))}
          </ul>
          {microBatches.length > 0 && (
            <p className="calibration-note">
              Micro-batch while reading prompts:{' '}
              {calibration.micro_batch
                ? `${calibration.micro_batch.toLocaleString()} chosen (at most 5% of generation given up, for at least 5 times as much prompt reading gained).`
                : 'none chosen, because 512 (the baseline) could not be measured.'}{' '}
              {microBatches
                .map((row) => `${row.micro_batch.toLocaleString()}: ${row.generation !== null ? `${row.generation.toFixed(1)} tok/s generating${signed(row.generation_change)}` : 'generation not measured'}, ${row.prompt !== null ? `${Math.round(row.prompt).toLocaleString()} tok/s reading prompts${signed(row.prompt_change)}` : 'prompt reading not measured'}`)
                .join('; ')}.
            </p>
          )}
          <p className="calibration-note">
            Measured {new Date(calibration.created_at).toLocaleString()} · {placementLabel(calibration.plan)} · {calibration.environment.power_source === 'ac' ? 'on AC power' : calibration.environment.power_source === 'battery' ? 'on battery' : 'power source unknown'} · {calibration.environment.cpu_topology}
          </p>
          <button type="button" className="activity-disclosure" aria-expanded={showMeasurements} onClick={() => setShowMeasurements((open) => !open)}>
            {showMeasurements ? 'Hide measurements' : 'Show every measurement'}
          </button>
          {showMeasurements && (
            <div className="calibration-table-wrap">
              <table className="calibration-table">
                <thead><tr><th>Threads</th><th>Idle waiting</th><th>Generation tok/s (p10–p90)</th><th>Prompt tok/s</th></tr></thead>
                <tbody>
                  {calibration.measurements.map((m, index) => (
                    <tr key={index}>
                      <td>{m.threads}</td>
                      <td>{m.poll === 0 ? 'sleep' : 'spin'}</td>
                      <td>{m.generation ? `${m.generation.median.toFixed(1)} (${m.generation.p10.toFixed(1)}–${m.generation.p90.toFixed(1)})` : '—'}</td>
                      <td>{m.prompt ? Math.round(m.prompt.median) : '—'}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </>
      )}
    </div>
  );
}
