type ModelLimit = { id: string; name: string; context_length: number; loaded?: boolean };

/** A warning when the context size asked for is larger than the relevant
 * model was trained for, or null. The relevant model is the saved default,
 * or the loaded one when no default is set. The runtime caps the window to
 * the model's limit at load time; this says so while the setting is being
 * edited rather than only in the runtime notes afterwards. */
export function contextSupportWarning(requested: number, models: ModelLimit[], defaultModelId?: string | null): string | null {
  if (!Number.isFinite(requested) || requested <= 0) return null;
  const model = (defaultModelId ? models.find((candidate) => candidate.id === defaultModelId) : undefined)
    ?? models.find((candidate) => candidate.loaded);
  if (!model || !model.context_length || requested <= model.context_length) return null;
  const n = (value: number) => value.toLocaleString('en-US');
  return `${model.name} supports at most ${n(model.context_length)} tokens, so it will load with ${n(model.context_length)} instead of ${n(requested)}. Models that support the full size still get it.`;
}
