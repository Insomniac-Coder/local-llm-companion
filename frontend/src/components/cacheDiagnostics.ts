export interface CacheDiagnostics {
  configured_context?: number;
  sampler?: { samples?: number; bytes_est?: number; cadence_secs?: number };
  // Legacy runtimes reported "auto" without implementing a budget manager.
  // Keep the response compatible, but never present these as enforced limits.
  budgets?: { ram?: string | null; vram?: string | null };
  kv_cache?: { owner?: string; usage_bytes?: number | null; persistent?: boolean };
  history?: { storage?: string; preserved_on_model_switch?: boolean };
  note?: string;
}

export function cacheDetails(cache: CacheDiagnostics) {
  const context = cache.configured_context;
  const measuredBytes = cache.kv_cache?.usage_bytes;
  return {
    capacity: typeof context === 'number' && Number.isFinite(context) && context > 0
      ? `${context.toLocaleString('en-US')} token capacity` : 'No context allocated',
    runtimeSize: typeof measuredBytes === 'number' && Number.isFinite(measuredBytes) && measuredBytes >= 0
      ? `${(measuredBytes / 1_048_576).toFixed(1)} MB` : 'Not measured',
    runtimeNote: 'Temporary inference cache inside llama-server; no per-session memory usage is reported.',
    history: 'Stored locally',
    historyNote: 'Conversations are saved in SQLite, separately from the model cache.',
    switchNote: 'Switching models recreates the temporary inference cache. Saved conversations, attachments and memory stay available.',
  };
}
