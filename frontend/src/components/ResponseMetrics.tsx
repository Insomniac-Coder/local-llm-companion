import type { OutputTiming } from '../services/outputTiming';

export default function ResponseMetrics({tps, live, timing, legacy, detailed}: {tps?:number|null; live?:boolean; timing?:OutputTiming|null; legacy?:boolean; detailed?:boolean}) {
  if (live) return tps != null ? <span className="tps" title="Approximate output tokens per second, starting at the first visible text. Processing, thinking and tool waits are excluded.">Output ≈{tps.toFixed(1)} tok/s</span> : null;
  if (timing?.basis === 'visible_output_v1') {
    // The runtime's own decode measurement is exact and includes hidden
    // reasoning tokens; the visible-delivery rate is the fallback.
    const engine = timing.engine_output_tps ?? null;
    const outputLabel = engine != null
      ? `Output ${engine.toFixed(1)} tok/s`
      : timing.output_tps == null ? 'Output rate —' : `Output ${timing.estimated ? '≈' : ''}${timing.output_tps.toFixed(1)} tok/s`;
    const outputTitle = engine != null
      ? `Decode speed measured by the model runtime over ${timing.predicted_tokens ?? '?'} generated tokens (hidden reasoning included). Prompt processing and tool waits are excluded.`
      : timing.output_tps == null ? 'Not enough time between visible chunks to calculate a meaningful output rate.' : `Output speed excludes preparation, hidden thinking and tool waits. ${timing.estimated ? 'Token count is estimated.' : 'Visible output token count comes from the model tokenizer.'}`;
    const cached = timing.cached_tokens ?? null;
    const drafted = timing.draft_tokens ?? null;
    return <span className="response-metrics">
      {timing.first_visible_ms != null && <span title="Time from the request until the first visible text, including preparation and any prior thinking.">First text {(timing.first_visible_ms / 1000).toFixed(1)}s</span>}
      {timing.thinking_ms != null && <span title="Time spent in a thinking phase explicitly reported by the model. Hidden reasoning text is not displayed.">Thinking {(timing.thinking_ms / 1000).toFixed(1)}s</span>}
      <span className="tps" title={outputTitle}>{outputLabel}</span>
      {detailed && timing.engine_prompt_tps != null && <span title="Prompt processing speed measured by the runtime for the tokens that were not already cached.">Prefill {timing.engine_prompt_tps.toFixed(0)} tok/s</span>}
      {detailed && cached != null && cached > 0 && <span title="Prompt tokens the runtime reused from its cache instead of recomputing them.">Cached {cached.toLocaleString()} tok</span>}
      {detailed && drafted != null && drafted > 0 && <span title="Speculative decoding: tokens drafted from the context and how many the model accepted. Output is identical either way.">Drafted {(timing.draft_accepted ?? 0).toLocaleString()}/{drafted.toLocaleString()}</span>}
      {detailed && <span title="Sum of visible-output spans, excluding processing and tool waits.">Output time {(timing.output_ms / 1000).toFixed(1)}s</span>}
      {detailed && <span title="Total turn time, including preparation, thinking and tools.">Total {(timing.total_ms / 1000).toFixed(1)}s</span>}
    </span>;
  }
  return legacy && tps != null ? <span className="legacy-tps" title="Saved by an older version. This overall rate includes preparation and is not comparable to the new output-only rate.">Legacy overall {tps.toFixed(1)} tok/s</span> : null;
}
