export type GenerationPhase = 'processing' | 'thinking' | 'responding';

export interface OutputTiming {
  basis: 'visible_output_v1';
  output_tps: number | null;
  estimated: boolean;
  token_basis: 'tokenizer' | 'character_estimate' | 'mixed';
  output_tokens: number;
  first_visible_ms: number | null;
  output_ms: number;
  thinking_ms: number | null;
  total_ms: number;
}

/** Approximate live display only; final rates come from backend timing. */
export class VisibleOutputMeter {
  private chars = 0;
  private completedMs = 0;
  private roundStart: number | null = null;
  private roundLast: number | null = null;

  pause() {
    if (this.roundStart !== null && this.roundLast !== null) {
      this.completedMs += Math.max(0, this.roundLast - this.roundStart);
    }
    this.roundStart = null;
    this.roundLast = null;
  }

  append(text: string, now: number): number | null {
    if (!text) return this.rate();
    this.roundStart ??= now;
    this.roundLast = now;
    this.chars += Array.from(text).length;
    return this.rate();
  }

  private rate(): number | null {
    const ms = this.completedMs + (this.roundStart !== null && this.roundLast !== null ? Math.max(0, this.roundLast - this.roundStart) : 0);
    return ms >= 200 && this.chars > 0 ? this.chars / 4 * 1000 / ms : null;
  }
}
