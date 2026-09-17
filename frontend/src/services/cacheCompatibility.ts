/** llama.cpp cannot create a context with an 8-bit value cache while Flash
 * Attention is off ("failed to create context"). Only Manual mode can switch
 * Flash Attention off; the other modes let the runtime enable it. `mode` is
 * the Performance mode as `performanceMode` reads it. */

type CacheSettings = {
  runtime?: { kv_cache?: string };
  hardware?: { flash_attention?: boolean };
};

const quantizedCache = (settings: CacheSettings | null | undefined) => (settings?.runtime?.kv_cache ?? 'f16') !== 'f16';

const manualWithoutFlashAttention = (settings: CacheSettings | null | undefined, mode: string) =>
  mode === 'manual' && !settings?.hardware?.flash_attention;

/** The 8-bit cache option cannot be picked: Manual mode has Flash Attention off. */
export function quantizedCacheUnavailable(settings: CacheSettings | null | undefined, mode: string): boolean {
  return manualWithoutFlashAttention(settings, mode);
}

/** Flash Attention cannot be switched off: the 8-bit cache is selected. */
export function flashAttentionRequired(settings: CacheSettings | null | undefined, mode: string): boolean {
  return mode === 'manual' && quantizedCache(settings) && !!settings?.hardware?.flash_attention;
}

/** The message for a combination that cannot load, or null. Reachable by
 * switching to Manual after choosing the 8-bit cache in another mode; Save
 * stays disabled until one of the two is changed. */
export function cacheConflict(settings: CacheSettings | null | undefined, mode: string): string | null {
  if (!manualWithoutFlashAttention(settings, mode) || !quantizedCache(settings)) return null;
  return 'An 8-bit KV cache needs Flash Attention, which is off in Manual mode. Turn Flash Attention on, or set KV cache precision to f16.';
}

export const FLASH_ATTENTION_REQUIRED_NOTE = 'Required by the 8-bit KV cache. Set KV cache precision to f16 to switch it off.';
export const FLASH_ATTENTION_CONFLICT_NOTE = 'Turn this on to keep the 8-bit KV cache selected above, or set that cache to f16.';
export const QUANTIZED_CACHE_UNAVAILABLE_NOTE = 'q8_0 needs Flash Attention, which is off in Manual mode.';
