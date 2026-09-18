/**
 * How a model calls tools, as its tool check measured on this machine
 * (`models/<folder>/tooling.json`, night decision 59). The check runs on the
 * model's first load, and again when the runtime or its chat template changes.
 */

export type ToolMethod = 'native' | 'text' | 'none';
export type ToolingState = 'current' | 'stale' | 'unchecked';

export interface ToolingCheck {
  name: string;
  passed: boolean;
  detail: string;
}

export interface ToolingProfile {
  version: number;
  method: ToolMethod;
  can_write: boolean;
  file_text: 'arguments' | 'raw_blocks' | 'none';
  checked_at: string;
  runtime: string;
  template: string;
  template_reports_tools?: boolean | null;
  checks: ToolingCheck[];
  duration_ms: number;
}

export type ToolingModel = {
  name: string;
  tooling?: ToolingProfile | null;
  tooling_state?: ToolingState | null;
};

/** The profile that describes the model now, or null. */
export function currentTooling(model: ToolingModel | null | undefined): ToolingProfile | null {
  return model?.tooling && model.tooling_state === 'current' ? model.tooling : null;
}

/** One plain sentence: what the check found. */
export function toolingSummary(profile: ToolingProfile): string {
  if (profile.method === 'native' && profile.can_write) return 'Calls tools natively through the runtime.';
  if (profile.method === 'text' && profile.can_write) return "Calls tools through the app's text format.";
  if (profile.method !== 'none') return 'Can read files with tools, but file text did not arrive intact, so code sessions stay read-only.';
  return 'Could not call tools in either format, so it gets chat and read-only code.';
}

/** The card's badge. */
export function toolingBadge(model: ToolingModel): { label: string; tone: 'info' | 'neutral' | 'warn' } {
  const profile = currentTooling(model);
  if (!profile) {
    return { label: model.tooling_state === 'stale' ? 'Tools: check again on load' : 'Tools: checked on first load', tone: 'neutral' };
  }
  if (!profile.can_write) return { label: 'Read-only code', tone: 'warn' };
  return { label: profile.method === 'native' ? 'Tools: native' : 'Tools: text format', tone: 'info' };
}

/** Said before a load that will run the check, so a longer load is expected. */
export function firstLoadNotice(model: ToolingModel | null | undefined): string | null {
  if (!model || model.tooling_state === 'current' || !model.tooling_state) return null;
  return model.tooling_state === 'stale'
    ? `Loading ${model.name} takes a few seconds longer this time: the app checks again how it calls tools, because the check, the runtime or its chat template changed.`
    : `The first load of ${model.name} takes a few seconds longer: the app checks how it calls tools.`;
}

/** Code sessions with this model may only read (decision 48). */
export function codeSessionsReadOnly(model: ToolingModel | null | undefined): boolean {
  const profile = currentTooling(model);
  return !!profile && !profile.can_write;
}

export const READ_ONLY_MODE_REASON = "Unavailable with this model: its tool check found that file text doesn't arrive intact, so code sessions can only read.";

/** Permission modes a code session can use with the loaded model. */
export function availablePermissionModes<M extends string>(modes: readonly M[], model: ToolingModel | null | undefined): M[] {
  if (!codeSessionsReadOnly(model)) return [...modes];
  return modes.filter((mode) => mode === 'ask');
}

const CHECK_LABELS: Record<string, string> = {
  native_call: 'Tool call through the runtime',
  native_file_text: 'File text through the runtime',
  text_call: "Tool call in the app's text format",
  text_file_text: "File text in the app's text format",
};

export function checkLabel(name: string): string {
  return CHECK_LABELS[name] ?? name;
}
