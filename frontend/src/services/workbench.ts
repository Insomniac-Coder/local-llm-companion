/** Updates one streaming message; never replaces an unrelated last message. */
export function updateMessage<T extends { id: string }>(messages: T[], id: string, update: (message: T) => T): T[] {
  return messages.map((message) => message.id === id ? update(message) : message);
}

/** Code sessions route like Claude Code: every message goes to the agent,
 *  which answers or acts within the permission mode; there is no separate
 *  classification request. Slash commands keep their own path. */
export function shouldStartAgent(mode: string, text: string): boolean {
  return mode === 'code' && !text.trim().startsWith('/');
}

export const PERMISSION_MODES = ['ask', 'accept_edits', 'plan', 'auto'] as const;
export type PermissionModeName = (typeof PERMISSION_MODES)[number];

/** The modes Shift+Tab cycles through. Auto is chosen deliberately, from the
 *  picker, never passed through on the way to another mode. */
export const KEYBOARD_PERMISSION_MODES = ['ask', 'accept_edits', 'plan'] as const;

/** Shift+Tab in a code session: the next permission mode, wrapping around.
 *  From Auto it returns to Ask. */
export function nextPermissionMode(mode: string): PermissionModeName {
  const index = (KEYBOARD_PERMISSION_MODES as readonly string[]).indexOf(mode);
  return index < 0 ? 'ask' : KEYBOARD_PERMISSION_MODES[(index + 1) % KEYBOARD_PERMISSION_MODES.length];
}

/** How long Shift+Tab waits for the next press before saving where it stopped. */
export const PERMISSION_MODE_SETTLE_MS = 450;

/** Saves the permission mode one request at a time, always the latest wanted
 *  mode. Changing the mode on the server can release actions waiting for
 *  approval, so a mode passed through while cycling is never saved, and two
 *  saves never race to leave the server on an older choice. */
export class PermissionModeSaver<M extends string, R extends {mode: M}> {
  private wanted: M;
  private saved: M;
  private timer: ReturnType<typeof setTimeout> | null = null;
  private chain: Promise<void> = Promise.resolve();
  private readonly save: (mode: M) => Promise<R>;
  /** The latest wanted mode was saved (`result`) or failed (`error`, `mode` is what the server keeps). */
  private readonly settled: (outcome: {mode: M; result?: R; error?: unknown}) => void;

  constructor(
    initial: M,
    save: (mode: M) => Promise<R>,
    settled: (outcome: {mode: M; result?: R; error?: unknown}) => void,
  ) {
    this.wanted = initial;
    this.saved = initial;
    this.save = save;
    this.settled = settled;
  }

  /** The mode the server is known to be in (after `settle`, the one in effect). */
  get mode(): M { return this.saved; }

  /** The server reported this mode (loading, or saved from Settings). */
  reset(mode: M) {
    this.cancelTimer();
    this.wanted = mode;
    this.saved = mode;
  }

  /** Save `mode` now; true when the server is in it afterwards. */
  request(mode: M): Promise<boolean> {
    this.wanted = mode;
    return this.settle();
  }

  /** Save `mode` once no other mode is wanted for `delayMs`. */
  schedule(mode: M, delayMs: number) {
    this.wanted = mode;
    this.cancelTimer();
    this.timer = setTimeout(() => { this.timer = null; void this.flush(); }, delayMs);
  }

  /** Save any scheduled mode now; true when the server is in the wanted mode. */
  settle(): Promise<boolean> {
    this.cancelTimer();
    return this.flush();
  }

  private cancelTimer() {
    if (this.timer) clearTimeout(this.timer);
    this.timer = null;
  }

  private flush(): Promise<boolean> {
    const wanted = this.wanted;
    const run = this.chain.then(async () => {
      const target = this.wanted;
      if (target === this.saved) return;
      try {
        const result = await this.save(target);
        this.saved = result.mode;
        if (this.wanted === target) {
          this.wanted = result.mode;
          this.settled({mode: result.mode, result});
        }
      } catch (error) {
        if (this.wanted === target) {
          this.wanted = this.saved;
          this.settled({mode: this.saved, error});
        }
      }
    });
    this.chain = run;
    return run.then(() => this.saved === wanted);
  }
}

/** The plan run waiting for the user's decision: the conversation's latest
 *  run, a plan that ended by presenting a plan, and not yet answered. A plan
 *  run that answered a question offers nothing to approve. */
export function planAwaitingApproval(runs: {id: string; state: string; mode?: string; plan_ready?: boolean}[], answered: ReadonlySet<string>): string | null {
  const latest = runs[runs.length - 1];
  return latest && latest.mode === 'plan' && latest.state === 'COMPLETED' && latest.plan_ready === true && !answered.has(latest.id) ? latest.id : null;
}

/** What approving a plan sends: a continuation the backend resolves to the
 *  planned task, with the plan itself in the conversation. */
export const APPROVE_PLAN_MESSAGE = 'Implement the plan';

/** Titles a session has before its first message names it. Code sessions start
 *  as "New task", which auto-titling did not recognise, so they kept it. */
export const PLACEHOLDER_TITLES = ['New chat', 'New Chat', 'New task'];

/** The title a session gets from its first message, or null when it already has
 *  one of its own (or the message has no words). A leading slash command is
 *  dropped, and a long message is cut at a word boundary with an ellipsis. */
export function autoTitle(currentTitle: string | undefined, userText: string): string | null {
  if (currentTitle === undefined || !PLACEHOLDER_TITLES.includes(currentTitle)) return null;
  const text = userText.replace(/^\/\w+\s*/, '').replace(/\s+/g, ' ').trim();
  if (!text) return null;
  if (text.length <= 48) return text;
  const cut = text.slice(0, 48);
  const space = cut.lastIndexOf(' ');
  return `${(space > 24 ? cut.slice(0, space) : cut).replace(/[\s,.;:!?-]+$/, '')}…`;
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
