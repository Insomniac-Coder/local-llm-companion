import type { OutputTiming } from '../services/outputTiming';

export default function ResponseMetrics({tps, live, timing, legacy, detailed}: {tps?:number|null; live?:boolean; timing?:OutputTiming|null; legacy?:boolean; detailed?:boolean}) {
  if (live) return tps != null ? <span className="tps" title="Approximate output tokens per second, starting at the first visible text. Processing, thinking and tool waits are excluded.">Output ≈{tps.toFixed(1)} tok/s</span> : null;
  if (timing?.basis === 'visible_output_v1') return <span className="response-metrics">
    {timing.first_visible_ms != null && <span title="Time from the request until the first visible text, including preparation and any prior thinking.">First text {(timing.first_visible_ms / 1000).toFixed(1)}s</span>}
    {timing.thinking_ms != null && <span title="Time spent in a thinking phase explicitly reported by the model. Hidden reasoning text is not displayed.">Thinking {(timing.thinking_ms / 1000).toFixed(1)}s</span>}
    <span className="tps" title={timing.output_tps == null ? 'Not enough time between visible chunks to calculate a meaningful output rate.' : `Output speed excludes preparation, hidden thinking and tool waits. ${timing.estimated ? 'Token count is estimated.' : 'Visible output token count comes from the model tokenizer.'}`}>
      {timing.output_tps == null ? 'Output rate —' : `Output ${timing.estimated ? '≈' : ''}${timing.output_tps.toFixed(1)} tok/s`}
    </span>
    {detailed && <span title="Sum of visible-output spans, excluding processing and tool waits.">Output time {(timing.output_ms / 1000).toFixed(1)}s</span>}
    {detailed && <span title="Total turn time, including preparation, thinking and tools.">Total {(timing.total_ms / 1000).toFixed(1)}s</span>}
  </span>;
  return legacy && tps != null ? <span className="legacy-tps" title="Saved by an older version. This overall rate includes preparation and is not comparable to the new output-only rate.">Legacy overall {tps.toFixed(1)} tok/s</span> : null;
}
