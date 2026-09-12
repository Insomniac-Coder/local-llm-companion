/** Updating one visible preference must preserve hidden and legacy preferences. */
export function updateSetting<T>(settings: T, path: string[], value: unknown): T {
  const copy = structuredClone(settings);
  let target: any = copy;
  for (const key of path.slice(0, -1)) {
    target[key] ??= {};
    target = target[key];
  }
  target[path[path.length - 1]] = value;
  return copy;
}

export function settingsSearchMatches(text: string, query: string): boolean {
  return text.toLowerCase().includes(query.trim().toLowerCase());
}

export function expertSectionOpen(manuallyOpen: boolean, text: string, query: string): boolean {
  return manuallyOpen || (!!query.trim() && settingsSearchMatches(text, query));
}
