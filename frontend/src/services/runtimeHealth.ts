/**
 * Whether Companion's backend is answering, from the outcome of each status
 * check. One failed check is not enough to call it down: a laptop waking from
 * sleep fails one on its own, and a single failure used to tell the owner to
 * restart an app that was still running (2026-09-18).
 */
export interface RuntimeHealth {
  /** null until a check has settled it. */
  up: boolean | null;
  /** Failed checks in a row. */
  misses: number;
}

/** Failed checks in a row before the backend is reported as not answering. */
export const MISSES_BEFORE_DOWN = 2;

/** How soon a failed check is repeated, instead of waiting for the next poll. */
export const RECHECK_MS = 3000;

export const initialHealth: RuntimeHealth = { up: null, misses: 0 };

export function nextHealth(previous: RuntimeHealth, answered: boolean): RuntimeHealth {
  if (answered) return { up: true, misses: 0 };
  const misses = previous.misses + 1;
  return { up: misses >= MISSES_BEFORE_DOWN ? false : previous.up, misses };
}

/** A failed check is repeated shortly, unless the backend is already down. */
export function shouldRecheck(health: RuntimeHealth): boolean {
  return health.misses > 0 && health.up !== false;
}

/** The notice line for a model server that ended by itself; the backend's
 *  report is how it ended and its last error, e.g. "exit code 1: …". */
export function modelStoppedDetail(report: string): string {
  const text = report.trim().replace(/[.;]+$/, '');
  return text ? `It ended by itself (${text}).` : 'It ended by itself.';
}
