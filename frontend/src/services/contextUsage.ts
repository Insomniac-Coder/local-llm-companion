import type { AgentEvent, ContextInfo } from './api';

export function contextDisplay(ctx: ContextInfo) {
  const agent = ctx.agent_context;
  const usage = agent?.usage;
  const pending = !!agent?.active && !usage;
  const reported = usage?.prompt_tokens != null && usage.prompt_tokens > 0;
  const tokens = pending ? null : usage ? (reported ? usage.prompt_tokens! : usage.estimated_tokens) : ctx.estimated_tokens;
  const limit = usage?.context_limit || ctx.limit || 0;
  const reserve = usage?.output_reserve ?? ctx.breakdown?.output_reserve ?? 0;
  // Measured against the room history may occupy (the window minus the
  // reply's reserve), the same measure automatic compaction uses, so the
  // gauge reads the threshold when compaction runs.
  const room = usage ? usage.history_room ?? 0 : ctx.history_room_tokens ?? 0;
  const percent = tokens == null ? 0 : !usage && ctx.usage_pct != null && room > 0 ? ctx.usage_pct : room > 0 ? tokens / room * 100 : limit > 0 ? tokens / limit * 100 : 0;
  const healthPercent = room > 0 ? percent : tokens != null && limit > 0 ? (tokens + reserve) / limit * 100 : 0;
  // The setting as it stands now, except while a run is going: that run read
  // the threshold when it started and keeps to it. Reading the last recorded
  // step instead showed the old number after the setting was changed.
  const compactAt = ctx.auto_compact === false
    ? 0
    : agent?.active
      ? usage?.compact_at_pct ?? ctx.compact_at_pct ?? 0
      : ctx.compact_at_pct ?? usage?.compact_at_pct ?? 0;
  return {
    tokens, limit, reserve, pending, estimated: !reported, room, compactAt,
    compactions: usage?.compactions ?? 0,
    label: agent?.active ? 'Agent input' : usage ? 'Last agent input' : 'Saved context',
    percent: Math.min(100, Math.max(0, percent)),
    percentLabel: percent > 0 && percent < 1 ? '<1%' : `${Math.round(percent)}%`,
    health: healthPercent >= 90 ? 'critical' : healthPercent >= 80 ? 'high' : healthPercent >= 60 ? 'moderate' : 'healthy',
  };
}

/** Root UI can update immediately from the live event while the same record
 * is persisted asynchronously. Never add counts from separate iterations. */
export function applyAgentContext(ctx: ContextInfo | null, event: AgentEvent, runId: string): ContextInfo | null {
  if (!ctx) return ctx;
  const active = !['COMPLETED', 'FAILED', 'CANCELLED'].includes(event.state);
  if (event.context_usage) return { ...ctx, agent_context: { run_id: runId, iteration: event.iteration, active, usage: event.context_usage } };
  if (ctx.agent_context?.run_id === runId && !active) return { ...ctx, agent_context: { ...ctx.agent_context, active: false } };
  return ctx;
}
