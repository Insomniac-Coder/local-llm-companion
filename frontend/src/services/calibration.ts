/** Per-model performance calibration: measured runtime profiles. */

export type ProfileName = 'fastest' | 'balanced' | 'light';
export type PerformanceMode = 'auto' | ProfileName | 'manual';

export interface Rate { median: number; p10: number; p90: number; samples: number }

export interface CalibrationProfile {
  name: ProfileName;
  threads: number;
  threads_batch: number;
  poll: number;
  priority: number;
  gpu_layers: number;
  generation_tps: number;
  prompt_tps: number;
  generation_share: number;
  free_cores_generating: number;
  description: string;
}

export interface CalibrationMeasurement {
  threads: number;
  poll: number;
  gpu_layers: number;
  prompt: Rate | null;
  generation: Rate | null;
}

export interface MicroBatchMeasurement {
  micro_batch: number;
  gpu_layers: number;
  expert_blocks_on_cpu: number;
  threads: number;
  prompt_tokens: number;
  prompt: Rate | null;
  generation: Rate | null;
  /** 0 for the first pass, 1 for the reversed second pass (absent in older calibrations). */
  pass?: number;
  load_without_mmap?: boolean;
}

/** One micro-batch size across its passes, with its change against 512 (the runtime's default). */
export interface MicroBatchSummary {
  micro_batch: number;
  gpu_layers: number;
  expert_blocks_on_cpu: number;
  generation: number | null;
  prompt: number | null;
  /** Percent change against 512, one decimal; null for 512 itself or when either side is unmeasured. */
  generation_change: number | null;
  prompt_change: number | null;
}

export interface CalibrationEnvironment {
  device_fingerprint: string;
  runtime_build: string;
  power_source: string;
  gpu_driver: string;
  cpu_topology: string;
}

export interface Calibration {
  id: string;
  model_id: string;
  model_key: string;
  created_at: string;
  environment: CalibrationEnvironment;
  plan: {
    placement: 'gpu' | 'hybrid' | 'cpu';
    threads: number[];
    gpu_layers: number;
    prompt_tokens: number;
    generated_tokens: number;
    repetitions: number;
    /** The fit's rules keeping expert weights in RAM (absent in older calibrations). */
    tensor_overrides?: string[];
    micro_batch?: number | null;
    load_without_mmap?: boolean;
  };
  measurements: CalibrationMeasurement[];
  profiles: CalibrationProfile[];
  regression: { previous_tps: number; current_tps: number; change_percent: number; previous_at: string } | null;
  /** Absent in calibrations saved before micro-batches were measured. */
  micro_batches?: MicroBatchMeasurement[];
  micro_batch?: number | null;
}

export interface CalibrationStatus {
  calibration: Calibration | null;
  current_environment: CalibrationEnvironment | null;
  comparable: boolean | null;
}

export const MODES: { value: PerformanceMode; label: string; description: string }[] = [
  { value: 'auto', label: 'Auto', description: 'Companion chooses settings for each model when it loads.' },
  { value: 'fastest', label: 'Fastest', description: "Each model's highest measured generation speed." },
  { value: 'balanced', label: 'Balanced', description: 'Nearly the fastest generation with fewer cores busy; prompts still use the fastest setting.' },
  { value: 'light', label: 'Light', description: 'Most room for other applications while keeping most of the speed, and yields the CPU to them.' },
  { value: 'manual', label: 'Manual', description: 'Set threads, GPU layers, batch and waiting behaviour yourself.' },
];

/** The saved mode, including saves from before modes existed. */
export function performanceMode(settings: { runtime_auto?: boolean; runtime?: { mode?: string } } | null | undefined): PerformanceMode {
  const mode = settings?.runtime?.mode;
  if (mode && MODES.some((option) => option.value === mode)) return mode as PerformanceMode;
  return settings?.runtime_auto === false ? 'manual' : 'auto';
}

export const PROFILE_ORDER: ProfileName[] = ['fastest', 'balanced', 'light'];

const cap = (word: string) => word.charAt(0).toUpperCase() + word.slice(1);

/** One line for a profile card, in measured terms. */
export function profileSummary(profile: CalibrationProfile): string {
  const threads = profile.threads === profile.threads_batch
    ? `${profile.threads} thread${profile.threads === 1 ? '' : 's'}`
    : `${profile.threads} generation / ${profile.threads_batch} prompt threads`;
  return `${profile.generation_tps.toFixed(1)} tok/s generating (${profile.generation_share}%) · ${Math.round(profile.prompt_tps)} tok/s prompts · ${threads} · ${profile.free_cores_generating} cores free`;
}

export function profileTitle(name: string): string {
  return cap(name);
}

/** Why a stored calibration is not being applied, or null when it is current. */
const DEFAULT_MICRO_BATCH = 512;

/** Each measured micro-batch size, averaged over its passes, with what it gained or gave up against 512. */
export function summarizeMicroBatches(measured: MicroBatchMeasurement[] | null | undefined): MicroBatchSummary[] {
  const runs = measured ?? [];
  const mean = (values: number[]) => (values.length ? values.reduce((sum, value) => sum + value, 0) / values.length : null);
  const sizes = [...new Set(runs.map((m) => m.micro_batch))].sort((a, b) => a - b);
  const rows = sizes.map((size) => {
    const ofSize = runs.filter((m) => m.micro_batch === size);
    return {
      micro_batch: size,
      gpu_layers: ofSize[0].gpu_layers,
      expert_blocks_on_cpu: ofSize[0].expert_blocks_on_cpu,
      generation: mean(ofSize.flatMap((m) => (m.generation ? [m.generation.median] : []))),
      prompt: mean(ofSize.flatMap((m) => (m.prompt ? [m.prompt.median] : []))),
    };
  });
  const base = rows.find((row) => row.micro_batch === DEFAULT_MICRO_BATCH);
  const change = (value: number | null, baseline: number | null | undefined, size: number) =>
    size === DEFAULT_MICRO_BATCH || value === null || !baseline ? null : Math.round(((value - baseline) / baseline) * 1000) / 10;
  return rows.map((row) => ({
    ...row,
    generation_change: change(row.generation, base?.generation, row.micro_batch),
    prompt_change: change(row.prompt, base?.prompt, row.micro_batch),
  }));
}

/** Where a calibration placed the model, in words. */
export function placementLabel(plan: Calibration['plan']): string {
  if (plan.placement === 'gpu') return 'every layer on the GPU';
  if (plan.placement === 'cpu') return 'CPU only';
  if (plan.tensor_overrides && plan.tensor_overrides.length > 0) return 'every layer on the GPU, some expert weights in RAM';
  return `${plan.gpu_layers} layers on the GPU, the rest on the CPU`;
}

export function staleReason(status: CalibrationStatus | null): string | null {
  if (!status?.calibration) return null;
  if (status.comparable !== false) return null;
  const before = status.calibration.environment;
  const now = status.current_environment;
  if (!now) return null;
  const changes = [
    before.runtime_build !== now.runtime_build && 'the runtime was rebuilt',
    before.power_source !== now.power_source && `power changed from ${before.power_source} to ${now.power_source}`,
    before.device_fingerprint !== now.device_fingerprint && 'the hardware differs',
  ].filter(Boolean);
  return `Measured under different conditions (${changes.join(', ') || 'conditions changed'}). Profiles are not applied until this model is calibrated again.`;
}

export async function getCalibration(modelId: string): Promise<CalibrationStatus> {
  const response = await fetch(`/api/models/${encodeURIComponent(modelId)}/calibration`);
  if (!response.ok) throw new Error(`Could not read calibration (${response.status}).`);
  return response.json();
}

/** Runs a calibration, reporting each stage; resolves with the result. */
export async function calibrateModel(modelId: string, onStage: (detail: string) => void, signal?: AbortSignal): Promise<Calibration> {
  const response = await fetch(`/api/models/${encodeURIComponent(modelId)}/calibrate`, { method: 'POST', signal });
  if (!response.ok || !response.body) throw new Error(`Calibration could not start (${response.status}).`);
  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  let buffer = '';
  let result: Calibration | null = null;
  let failure: string | null = null;
  const handle = (frame: string) => {
    let event = '';
    const data: string[] = [];
    for (const line of frame.replace(/\r/g, '').split('\n')) {
      if (line.startsWith('event:')) event = line.slice(6).trim();
      else if (line.startsWith('data:')) data.push(line.slice(5).replace(/^ /, ''));
    }
    const payload = data.join('\n');
    if (event === 'stage') {
      try { onStage(JSON.parse(payload).detail); } catch { /* malformed stage: ignore */ }
    } else if (event === 'done') {
      result = JSON.parse(payload) as Calibration;
    } else if (event === 'error') {
      failure = payload || 'Calibration failed.';
    }
  };
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    buffer += decoder.decode(value, { stream: true });
    let index: number;
    while ((index = buffer.indexOf('\n\n')) >= 0) {
      handle(buffer.slice(0, index));
      buffer = buffer.slice(index + 2);
    }
  }
  if (failure) throw new Error(failure);
  if (!result) throw new Error('Calibration ended without a result.');
  return result;
}
