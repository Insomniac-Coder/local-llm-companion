import { useId, useMemo, useState } from 'react';
import { chartSegments, formatReading, isReading, timeLabel, type ResourceSample } from './resourceTelemetry';

interface Props {
  samples: ResourceSample[];
  latest: ResourceSample | null;
  field: keyof ResourceSample;
  label: string;
  detail: string;
  maximum: number;
  unit: '%' | 'GB';
  tone: 'cpu' | 'ram' | 'gpu' | 'vram';
  seconds: number;
}

// Charts are drawn in the neutral ink colour; only the newest reading is lit
// red, because it is the one live value on the page.
export default function ResourceChart({ samples, latest, field, label, detail, maximum, unit, tone, seconds }: Props) {
  const gradient = useId().replace(/:/g, '');
  const [inspection, setInspection] = useState<number | null>(null);
  const end = samples[samples.length - 1]?.ts ?? latest?.ts ?? 0;
  const start = end - seconds;
  const segments = useMemo(() => chartSegments(samples, field, maximum, start, end), [samples, field, maximum, start, end]);
  const points = segments.flat();
  const latestValue = latest?.[field];
  const selected = inspection === null ? null : points[Math.min(inspection, points.length - 1)];
  const peak = points.length ? Math.max(...points.map((point) => point.value)) : null;
  const digits = unit === 'GB' ? 1 : 0;
  const fill = isReading(latestValue) ? Math.min(100, latestValue / Math.max(1, maximum) * 100) : null;
  const newest = points[points.length - 1];
  const inspectPointer = (event: React.PointerEvent<HTMLDivElement>) => {
    const bounds = event.currentTarget.getBoundingClientRect();
    const x = (event.clientX - bounds.left) / bounds.width * 600;
    let index = 0;
    for (let i = 1; i < points.length; i++) if (Math.abs(points[i].x - x) < Math.abs(points[index].x - x)) index = i;
    setInspection(index);
  };
  return (
    <article className={`rd-chart rd-chart-${tone}`}>
      <header className="rd-chart-header">
        <div><h2>{label}</h2><p>{detail}</p></div>
        <div className="rd-reading">{formatReading(latestValue, digits)}<span>{unit}</span></div>
      </header>
      <div className="rd-capacity" aria-label={`${label}: ${formatReading(latestValue, digits)} ${unit}`}><span style={{ width: `${fill ?? 0}%` }} /></div>
      <div className="rd-inspection" aria-live="off">{selected ? <><time>{timeLabel(selected.ts)}</time><strong>{formatReading(selected.value, digits)} {unit}</strong></> : <><span>{points.length ? 'Hover or use arrow keys to inspect' : 'Waiting for a reading'}</span><span>{peak === null ? '' : `Peak ${formatReading(peak, digits)} ${unit}`}</span></>}</div>
      <div className="rd-plot-layout">
        <div className="rd-axis-y" aria-hidden="true"><span>{formatReading(maximum, maximum < 10 ? 1 : 0)}</span><span>{formatReading(maximum / 2, maximum < 10 ? 1 : 0)}</span><span>0</span></div>
        <div className="rd-plot" tabIndex={points.length ? 0 : undefined} role="img"
          aria-label={`${label} history over ${seconds / 60} minutes. Current ${formatReading(latestValue, digits)} ${unit}. Peak ${formatReading(peak, digits)} ${unit}. Use left and right arrow keys to inspect readings.`}
          onPointerMove={points.length ? inspectPointer : undefined} onPointerLeave={() => setInspection(null)} onBlur={() => setInspection(null)}
          onKeyDown={(event) => {
            if (!['ArrowLeft', 'ArrowRight', 'Home', 'End', 'Escape'].includes(event.key)) return;
            event.preventDefault();
            if (event.key === 'Escape') setInspection(null);
            else if (event.key === 'Home') setInspection(0);
            else if (event.key === 'End') setInspection(points.length - 1);
            else setInspection((value) => Math.max(0, Math.min(points.length - 1, (value ?? points.length - 1) + (event.key === 'ArrowLeft' ? -1 : 1))));
          }}>
          <svg viewBox="0 0 600 144" preserveAspectRatio="none" aria-hidden="true">
            <defs><linearGradient id={gradient} x1="0" x2="0" y1="0" y2="1"><stop offset="0" stopColor="currentColor" stopOpacity=".16" /><stop offset="1" stopColor="currentColor" stopOpacity="0" /></linearGradient></defs>
            <g className="rd-grid">{[4, 72, 140].map((y) => <line key={`y${y}`} x1="0" x2="600" y1={y} y2={y} />)}{[0, 150, 300, 450, 600].map((x) => <line key={`x${x}`} x1={x} x2={x} y1="4" y2="140" />)}</g>
            {segments.map((segment, index) => {
              const line = segment.map((point) => `${point.x.toFixed(1)},${point.y.toFixed(1)}`).join(' ');
              return <g key={index}><polygon points={`${segment[0].x},140 ${line} ${segment[segment.length - 1].x},140`} fill={`url(#${gradient})`} /><polyline className="rd-line" points={line} /></g>;
            })}
            {selected && <line className="rd-crosshair" x1={selected.x} x2={selected.x} y1="4" y2="140" />}
          </svg>
          {newest && <span className="rd-dot" style={{ left: `${newest.x / 6}%`, top: `${newest.y / 1.44}%` }} aria-hidden="true" />}
          {selected && selected !== newest && <span className="rd-dot inspect" style={{ left: `${selected.x / 6}%`, top: `${selected.y / 1.44}%` }} aria-hidden="true" />}
          {!points.length && <div className="rd-no-data">{latest && !isReading(latestValue) ? 'This sensor is not available' : 'Collecting telemetry'}</div>}
        </div>
      </div>
      <div className="rd-axis-x" aria-hidden="true"><span>{seconds / 60} min ago</span><span>{seconds / 120} min ago</span><span>Latest</span></div>
      {selected && <span className="rd-sr-only" aria-live="polite">{timeLabel(selected.ts)}: {formatReading(selected.value, digits)} {unit}</span>}
    </article>
  );
}
