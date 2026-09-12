/** Updates one streaming message; never replaces an unrelated last message. */
export function updateMessage<T extends { id: string }>(messages: T[], id: string, update: (message: T) => T): T[] {
  return messages.map((message) => message.id === id ? update(message) : message);
}

export type CodeIntent = 'ask' | 'plan' | 'agent';

export function shouldStartAgent(mode: string, intent: string, text: string): boolean {
  return mode === 'code' && (intent === 'plan' || intent === 'agent') && !text.trim().startsWith('/');
}

/** A saved default selects a model on startup; it never switches a loaded model. */
export function selectAvailableModel(models: {id:string;loaded?:boolean}[], current: string, preferred: string): string {
  if (models.some((model) => model.id === current)) return current;
  return models.find((model) => model.loaded)?.id
    ?? models.find((model) => model.id === preferred)?.id
    ?? models[0]?.id ?? '';
}

export function matchesShortcut(event: {key:string;ctrlKey:boolean;metaKey:boolean;altKey:boolean;shiftKey:boolean}, binding: string): boolean {
  const parts = binding.toLowerCase().split('+').map((part) => part.trim());
  const key = parts.pop();
  if (!parts.some((part) => ['ctrl','control','meta','cmd','alt'].includes(part))) return false;
  return event.key.toLowerCase() === key
    && event.ctrlKey === (parts.includes('ctrl') || parts.includes('control'))
    && event.metaKey === (parts.includes('meta') || parts.includes('cmd'))
    && event.altKey === parts.includes('alt') && event.shiftKey === parts.includes('shift');
}

export function validatedPanelWidth(value: unknown): number {
  const width = Number(value);
  return Number.isFinite(width) && width >= 320 ? Math.min(640, width) : 400;
}
/** One source for utility navigation and keyboard search. */
export const WORKBENCH_DESTINATIONS = [
  { id: 'models', label: 'Models' },
  { id: 'resources', label: 'Resources' },
  { id: 'system', label: 'Runtime & diagnostics' },
  { id: 'tools', label: 'Tools & plugins' },
  { id: 'settings', label: 'Settings' },
] as const;
