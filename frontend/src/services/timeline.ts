import type { AgentEvent } from './api';

const TERMINAL_STATES = ['COMPLETED', 'FAILED', 'CANCELLED'];
const GENERIC_PHASES = ['Preparing the first action', 'Reviewing the result and preparing the next action'];

/** The rows an activity timeline shows for a run's events.
 *  - context and streaming deltas are not rows;
 *  - a tool's result replaces the row of its start;
 *  - a start with no result in a run that ended says so;
 *  - only the latest "preparing the next action" row is kept;
 *  - a progress note that the run's final answer repeats is not shown twice. The
 *    runner records the model's last reply as progress and then as the answer,
 *    so a question's answer used to appear twice in a row. */
export function visibleTimelineEvents(events: AgentEvent[]): AgentEvent[] {
  const terminal = events.some((event) => TERMINAL_STATES.includes(event.state));
  const rows = events.reduce<AgentEvent[]>((items, event) => {
    if (event.kind === 'context' || event.kind === 'thought_delta') return items;
    if (event.kind === 'tool_result' || event.kind === 'tool_error') {
      const pending = items.findIndex((item) => item.kind === 'tool_started'
        && item.iteration === event.iteration && item.tool === event.tool);
      if (pending >= 0) { items[pending] = event; return items; }
    }
    items.push(event);
    return items;
  }, []).map((event): AgentEvent => terminal && event.kind === 'tool_started' ? {
    ...event, kind: 'tool_error', state: 'OBSERVING', diff: undefined,
    message: 'No result was recorded before this run ended. Inspect the current file or output before retrying.',
  } : event);
  const finals = rows.filter((event) => event.kind === 'final').map((event) => (event.message ?? '').trim());
  return rows.filter((event, index) => {
    // "Preparing the next action" rows say only that a step is under way: the
    // latest one shows live progress, earlier ones are noise between the steps.
    if (event.kind === 'status' && GENERIC_PHASES.some((phase) => (event.message ?? '').startsWith(phase))) {
      return index === rows.length - 1;
    }
    if (event.kind !== 'thought') return true;
    const note = (event.message ?? '').trim();
    // Progress notes are cut at 1,200 characters, so a repeat can be a prefix.
    return !note || !finals.some((answer) => answer === note || (note.length >= 80 && answer.startsWith(note)));
  });
}

/** The model step a row belongs to ("Step 3"), or none for rows outside the
 *  steps (the task itself). Rows were numbered by position, so a five-step
 *  question read "Step 15" by its answer. */
export function stepLabel(event: AgentEvent): string {
  return event.iteration > 0 ? `Step ${event.iteration}` : '';
}
