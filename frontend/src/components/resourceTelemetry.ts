export interface ResourceSample {
  ts: number;
  cpu_pct: number | null;
  ram_used_gb: number | null;
  ram_total_gb: number | null;
  gpu_pct: number | null;
  vram_used_gb: number | null;
  vram_total_gb: number | null;
  gpu_temp_c: number | null;
  gpu_power_w: number | null;
}

export const isReading = (value: unknown): value is number => typeof value === 'number' && Number.isFinite(value) && value >= 0;
export const formatReading = (value: unknown, digits = 0) => isReading(value) ? value.toFixed(digits) : '—';

/** Missing readings break the line; they are never rendered as zero usage. */
export function chartSegments(samples: ResourceSample[], field: keyof ResourceSample, maximum: number, start: number, end: number) {
  const segments: { x: number; y: number; value: number; ts: number }[][] = [];
  let segment: typeof segments[number] = [];
  for (const sample of samples) {
    const value = sample[field];
    if (!isReading(value) || !isReading(sample.ts)) {
      if (segment.length) segments.push(segment);
      segment = [];
      continue;
    }
    const x = Math.max(0, Math.min(600, (sample.ts - start) / Math.max(1, end - start) * 600));
    const y = 140 - Math.max(0, Math.min(1, value / Math.max(1, maximum))) * 136;
    segment.push({ x, y, value, ts: sample.ts });
  }
  if (segment.length) segments.push(segment);
  return segments;
}

export function timeLabel(ts: number) {
  return new Date(ts * 1000).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' });
}
