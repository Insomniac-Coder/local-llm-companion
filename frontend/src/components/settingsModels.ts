export type ModelListState = 'loading' | 'ready' | 'error';
export type ModelOption = { value: string; label: string };

/** Keep a saved ID representable even while discovery is unavailable. */
export function defaultModelOptions(
  models: ReadonlyArray<{ id: string; name: string }>,
  selectedId: string,
  state: ModelListState,
): ModelOption[] {
  const detected = [...new Map(models.map((model) => [model.id, model])).values()]
    .sort((a, b) => a.name.localeCompare(b.name));
  const options: ModelOption[] = [{ value: '', label: 'Automatic selection' }];
  if (selectedId && !detected.some((model) => model.id === selectedId)) {
    options.push({
      value: selectedId,
      label: state === 'ready'
        ? 'Previously selected model unavailable'
        : state === 'loading' ? 'Saved default — checking availability…' : 'Saved default — model list unavailable',
    });
  }
  return options.concat(detected.map((model) => ({ value: model.id, label: model.name })));
}
