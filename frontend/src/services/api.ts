import type { GenerationPhase, OutputTiming } from './outputTiming';
export const API = '';

export interface ModelMeta {
  id: string;
  name: string;
  quantization: string;
  parameters: string;
  context_length: number;
  vision: boolean;
  tool_calling: boolean;
  /** 'runtime' when llama.cpp reported it for this file, 'template' when only the chat template suggests it. */
  tool_support_source?: 'runtime' | 'template' | null;
  supports_reasoning?: boolean;
  /** Structural reasons this file may not load or chat correctly (advisory). */
  load_issues?: string[];
  loaded: boolean;
}

export interface Conversation {
  id: string;
  title: string;
  model_id: string;
  created_at: string;
  mode?: string;
  workspace?: string;
  reasoning_default?: boolean;
  search_default?: boolean;
  last_model?: string;
}

export interface ChatMessage {
  id: string;
  conversation_id: string;
  role: 'user' | 'assistant' | 'tool';
  content: string;
  created_at: string;
  /** Verified execution events; legacy messages may not have a journal. */
  activities?: AgentEvent[];
}

async function req(path: string, init?: RequestInit) {
  const r = await fetch(path, init);
  if (!r.ok) {
    // Backend returns {error, hint} (§52); surface both, keep the status.
    try {
      const j = await r.json();
      const e: any = new Error(j.error ? `${j.error} — ${j.hint ?? ''}`.trim() : `request failed: ${r.status}`);
      e.status = r.status;
      throw e;
    } catch (e) {
      if (e instanceof Error && !e.message.startsWith('{')) throw e;
      throw new Error(`request failed: ${r.status}`);
    }
  }
  return r.json();
}

export async function listModels(): Promise<ModelMeta[]> {
  return req('/api/models');
}

export async function loadModel(id: string, force = false) {
  return req(`/api/models/load${force ? '?force=true' : ''}`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ id }),
  });
}

export async function unloadModels() {
  return req('/api/models/unload', { method: 'POST' });
}

export async function scanModels(): Promise<{ registered: number; warnings: string[] }> {
  return req('/api/models/scan', { method: 'POST' });
}

export interface InferenceStatus {
  runtime_notice?: string | null;
  engine: string;
  running: boolean;
  base_url: string | null;
  model: string | null;
  context_size: number;
  binary_found: boolean;
  last_error: string | null;
}

export async function inferenceStatus(): Promise<InferenceStatus> {
  return req('/api/inference/status');
}

export async function inferenceStart(model_id: string, force = false) {
  return req(`/api/inference/start${force ? '?force=true' : ''}`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ model_id }),
  });
}

export async function inferenceStop() {
  return req('/api/inference/stop', { method: 'POST' });
}

export interface ModelDetail {
  metadata: ModelMeta & { architecture: string; dir: string; projector_file: string | null; supports_reasoning?: boolean };
  estimates: {
    file_gb: number | null;
    gguf_present: boolean;
    projector_present: boolean;
    kv_cache_gb: number;
    total_need_gb: number | null;
    fits_vram: boolean | null;
    fits_system: boolean | null;
    compat_warnings: string[];
  };
  recommended: { gpu_layers_percent: number; context: number; batch: number; note: string } | null;
}

export async function modelDetail(id: string): Promise<ModelDetail> {
  return req(`/api/models/${id}`);
}

export async function deleteModel(id: string) {
  const r = await fetch(`/api/models/${id}`, { method: 'DELETE' });
  if (!r.ok) {
    try {
      const j = await r.json();
      throw new Error(j.error ?? `delete failed: ${r.status}`);
    } catch (e) {
      if (e instanceof Error && e.message !== 'Unexpected end of JSON input') throw e;
      throw new Error(`delete failed: ${r.status}`);
    }
  }
  return r.json();
}

export interface DownloadInfo {
  id: string;
  url: string;
  total_bytes: number | null;
  downloaded_bytes: number;
  status: 'downloading' | 'paused' | 'verifying' | 'completed' | 'failed' | 'cancelled';
  error: string | null;
}

export async function listDownloads(): Promise<DownloadInfo[]> {
  return req('/api/models/downloads');
}

export async function startDownload(id: string, url: string, sha256?: string): Promise<DownloadInfo> {
  return req('/api/models/downloads', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ id, url, sha256: sha256 || null }),
  });
}

export async function downloadAction(id: string, action: 'pause' | 'resume' | 'cancel') {
  return req(`/api/models/downloads/${id}/${action}`, { method: 'POST' });
}

export async function listConversations(): Promise<Conversation[]> {
  return req('/api/conversations');
}

export async function createConversation(
  title: string,
  model_id: string,
  extra?: { mode?: string; workspace?: string; reasoning_default?: boolean; search_default?: boolean },
): Promise<Conversation> {
  return req('/api/conversations', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ title, model_id, ...extra }),
  });
}

export async function getMessages(conversationId: string): Promise<ChatMessage[]> {
  return req(`/api/conversations/${conversationId}/messages`);
}

export async function postMessage(conversationId: string, role: ChatMessage['role'], content: string): Promise<ChatMessage> {
  return req(`/api/conversations/${conversationId}/messages`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ role, content }),
  });
}

export async function editMessage(conversationId: string, mid: string, content: string) {
  return req(`/api/conversations/${conversationId}/messages/${mid}`, {
    method: 'PATCH',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ content }),
  });
}

export interface ContextInfo {
  /** The window the loaded model actually received. */
  limit: number;
  /** The context size saved in settings, before the loader fitted it. */
  configured_limit?: number;
  /** 'fit' shrinks the context to GPU memory; 'requested' keeps it as saved. */
  context_fit?: string;
  /** Why the loaded window differs from the saved one, in the loader's words. */
  limit_note?: string | null;
  estimated_tokens: number;
  messages_total: number;
  messages_kept: number;
  messages_dropped: number;
  attachments: { count: number; chars: number };
  breakdown?: { system: number; conversation: number; attachments: number; tools: number; memory: number; output_reserve: number };
  used_with_reserve?: number;
  /** Saved history as a share of the room a request gives it: the measure automatic compaction uses. */
  usage_pct?: number;
  history_room_tokens?: number | null;
  auto_compact?: boolean;
  compact_at_pct?: number;
  health?: string;
  measurement?: 'saved_history_estimate';
  agent_context?: { run_id: string; iteration: number; active: boolean; usage: AgentContextUsage | null } | null;
}

export interface AgentContextUsage {
  estimated_tokens: number;
  prompt_tokens: number | null;
  generated_tokens: number | null;
  context_limit: number;
  output_reserve: number;
  turns: number;
  pruned_turns: number;
  images: number;
  phase: 'request' | 'response';
  /** Times this run summarized earlier turns to stay within the window. */
  compactions?: number;
  /** Tokens the transcript may occupy; automatic compaction measures against it. */
  history_room?: number;
  /** Automatic compaction threshold as a share of history_room; 0 when off. */
  compact_at_pct?: number;
}

export async function getContext(conversationId: string): Promise<ContextInfo> {
  return req(`/api/conversations/${conversationId}/context`);
}

export async function uploadAttachment(conversationId: string, file: File) {
  if (file.type.startsWith('image/')) {
    const buf = new Uint8Array(await file.arrayBuffer());
    if (buf.length > 8_000_000) throw new Error('Image too large (max 8 MB).');
    let bin = '';
    for (let i = 0; i < buf.length; i++) bin += String.fromCharCode(buf[i]);
    return req(`/api/conversations/${conversationId}/attachments`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ filename: file.name, mime: file.type, content_base64: btoa(bin) }),
    });
  }
  // Stage 28: office/binary docs travel as base64 for server-side sniffing.
  const office = /\.(pdf|docx|xlsx|pptx|zip|odt|ods|odp|ppt|doc|xls)$/i.test(file.name)
    || (file.type === 'application/octet-stream' || file.type === 'application/pdf');
  if (office) {
    const buf = new Uint8Array(await file.arrayBuffer());
    if (buf.length > 8_000_000) throw new Error('File too large (max 8 MB).');
    let bin = '';
    const CH = 0x8000;
    for (let i = 0; i < buf.length; i += CH) {
      bin += String.fromCharCode(...buf.subarray(i, i + CH));
    }
    return req(`/api/conversations/${conversationId}/attachments`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ filename: file.name, mime: file.type || 'application/octet-stream', content_base64: btoa(bin) }),
    });
  }
  const text = await file.text();
  if (text.length > 5_000_000) throw new Error('File too large (max 5 MB text).');
  return req(`/api/conversations/${conversationId}/attachments`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ filename: file.name, mime: file.type || 'text/plain', content: text }),
  });
}

export interface AttachmentInfo {
  id: string;
  filename: string;
  mime: string;
  size_bytes: number;
  text_excerpt: string;
  kind: string;
  status?: string;
}

export async function listAttachments(conversationId: string): Promise<AttachmentInfo[]> {
  return req(`/api/conversations/${conversationId}/attachments`);
}

export async function deleteAttachment(conversationId: string, aid: string) {
  const r = await fetch(`/api/conversations/${conversationId}/attachments/${aid}`, { method: 'DELETE' });
  if (!r.ok) throw new Error(`delete failed: ${r.status}`);
  return r.json();
}

export interface StreamUsage {
  message_id?: string;
  timing?: OutputTiming | null;
  prompt_tokens: number;
  generated_tokens: number;
  stopped?: boolean;
  /** The runtime hit the output limit before the reply finished. */
  truncated?: boolean;
  reasoning?: string;
  sources?: number;
  vision?: string;
  /** Measured output tok/s (null when no inference ran, e.g. commands). */
  gen_tps?: number | null;
  ttft_ms?: number;
  gen_ms?: number;
  /** Stage 31 in-chat file-read rounds used for this turn. */
  tool_rounds?: number;
  command?: { type: string; conversation_id?: string; run_id?: string; text?: string };
}

/** POST /api/chat/stop: abort the running sidecar request (frees its GPU
 *  slot) and keep whatever was streamed so far. Safe when idle. */
export async function stopChat(): Promise<{ stopped: boolean; chars_kept: number }> {
  const r = await fetch('/api/chat/stop', { method: 'POST' });
  return r.json();
}

/** POST /api/chat with SSE streaming (§58): `token` deltas, `status`
 *  notices, then `done` with usage (+optional UI command action). */
export async function streamChat(
  message: string,
  conversationId: string | null,
  cbs: {
    onToken: (t: string) => void;
    /** A tool action the model wrote: shown as a status line, never as text. */
    onAction?: (action: { state: 'started' | 'finished' | 'incomplete'; name: string | null }) => void;
    /** The visible reply was cut back (a rejected round): show this text instead. */
    onReplace?: (text: string) => void;
    /** Native reasoning text as the model produces it (never persisted). */
    onReasoning?: (t: string) => void;
    onStatus?: (s: string) => void;
    onPhase?: (phase: {phase: GenerationPhase; round?:number}) => void;
    onActivity?: (event: AgentEvent) => void;
    onDone?: (u: StreamUsage) => void;
    onError?: (msg: string) => void;
  },
  signal: AbortSignal,
  opts?: { reasoning?: boolean; search?: boolean },
): Promise<void> {
  const r = await fetch('/api/chat', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({
      message,
      conversation_id: conversationId ?? '',
      ...(opts?.reasoning !== undefined ? { reasoning: opts.reasoning } : {}),
      ...(opts?.search !== undefined ? { search: opts.search } : {}),
    }),
    signal,
  });
  if (!r.ok || !r.body) {
    try {
      const j = await r.json();
      throw new Error(j.error ?? `chat failed: ${r.status}`);
    } catch (e) {
      if (e instanceof Error && e.message !== 'Unexpected end of JSON input') throw e;
      throw new Error(`chat failed: ${r.status}`);
    }
  }
  const reader = r.body.getReader();
  const dec = new TextDecoder();
  let buf = '';
  const handleFrame = (frame: string) => {
    let event = 'message';
    // SSE joins consecutive `data:` lines with a newline. A delta that IS a
    // newline arrives as two empty data lines; joining with a "was there
    // data before?" check dropped it, which glued code fences to the code
    // that followed them.
    const parts: string[] = [];
    for (const line of frame.split('\n')) {
      if (line.startsWith('event:')) event = line.slice(6).trim();
      else if (line.startsWith('data:')) {
        // SSE framing puts one optional space after `data:` — strip exactly
        // that. Never trim further: token deltas like " how" carry meaning
        // in their leading space (trimStart here ate all spaces).
        let v = line.slice(5);
        if (v.startsWith(' ')) v = v.slice(1);
        parts.push(v);
      }
    }
    const data = parts.join('\n');
    if (event === 'token') cbs.onToken(data);
    else if (event === 'action') {
      try { if (cbs.onAction) cbs.onAction(JSON.parse(data)); } catch { /* a malformed action frame changes nothing visible */ }
    }
    else if (event === 'replace') { if (cbs.onReplace) cbs.onReplace(data); }
    else if (event === 'reasoning') { if (cbs.onReasoning) cbs.onReasoning(data); }
    else if (event === 'status' && cbs.onStatus) cbs.onStatus(data);
    else if (event === 'phase' && cbs.onPhase) {
      try { const phase = JSON.parse(data); if (['processing','thinking','responding','compacting'].includes(phase.phase)) cbs.onPhase(phase); } catch { /* unknown phase leaves the current public status unchanged */ }
    }
    else if (event === 'activity' && cbs.onActivity) {
      try { cbs.onActivity(JSON.parse(data)); } catch { /* malformed frames are not execution evidence */ }
    }
    else if (event === 'done' && cbs.onDone) {
      try {
        cbs.onDone(JSON.parse(data));
      } catch { /* usage optional */ }
    } else if (event === 'error' && cbs.onError) cbs.onError(data);
  };
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    buf += dec.decode(value, { stream: true });
    let idx: number;
    while ((idx = buf.indexOf('\n\n')) >= 0) {
      handleFrame(buf.slice(0, idx));
      buf = buf.slice(idx + 2);
    }
  }
  if (buf.trim()) handleFrame(buf);
}

export async function systemInfo() {
  return req('/api/system');
}

export interface Workspace {
  id: string;
  name: string;
  path: string;
  build_system: string;
  created_at: string;
}

export async function listWorkspaces(): Promise<Workspace[]> {
  return req('/api/workspaces');
}

export async function createWorkspace(name: string, path: string, create = false): Promise<Workspace> {
  return req('/api/workspaces', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ name, path, create }),
  });
}

export async function pickProjectFolder(): Promise<string | null> {
  const result = await req('/api/system/pick-folder', { method: 'POST' }) as { path: string | null };
  return result.path;
}

export async function patchConversation(id: string, patch: Record<string, unknown>) {
  return req(`/api/conversations/${id}`, {
    method: 'PATCH',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(patch),
  });
}

export async function forkConversation(id: string, title?: string) {
  return req(`/api/conversations/${id}/fork`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ title: title ?? '' }),
  });
}

export async function shareConversation(
  id: string,
  target_id: string,
  turns = 6,
  note = '',
  include?: { messages?: boolean; summary?: boolean; attachments?: boolean; memory?: boolean },
) {
  return req(`/api/conversations/${id}/share`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ target_id, turns, note, ...(include ?? {}) }),
  });
}

export interface SessionInfo {
  id: string;
  title: string;
  mode: string;
  workspace: string;
  model_id: string;
  residency: string;
  activity?: string;
  priority?: string;
  related_to?: string;
}

export async function listSessions(): Promise<{ sessions: SessionInfo[] }> {
  return req('/api/sessions');
}

export interface CommandItem {
  name: string;
  aliases: string[];
  description: string;
  category: string;
  risk: string;
  usage: string;
}

export async function listCommands(q: string): Promise<CommandItem[]> {
  return req(`/api/commands?q=${encodeURIComponent(q)}`);
}

export async function getSettings(): Promise<any> {
  return req('/api/settings');
}

export async function putSettings(s: any) {
  return req('/api/settings', {
    method: 'PUT',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(s),
  });
}

export interface ResolvedRuntimePolicy {
  mode: string; architecture: string; weights_quantization: string;
  requested_context: number; effective_context: number;
  cache_type_k: string; cache_type_v: string; flash_attention: string;
  kv_offload: string; threads: number; gpu_layers: number;
  /** 0 = the runtime's own default batch sizes. */
  batch_size: number;
  /** Minimum KV chunk reused after a prompt diverges; 0 = off. */
  cache_reuse?: number;
  /** `--spec-type` value, or "none". */
  speculative?: string;
  /** Planned residency from measured memory: gpu | hybrid | oversubscribed | cpu | unknown. */
  placement?: string;
  /** Context ceiling applied on the CPU fallback, sized from free RAM. */
  cpu_context_cap?: number;
  cache_rebuild: string; notes: string[];
}

export interface RuntimePolicy {
  running: boolean; model_id: string | null; model_name: string | null;
  active: ResolvedRuntimePolicy | null; next: ResolvedRuntimePolicy;
  applies_on: 'next_model_load';
}

export async function getRuntimePolicy(modelId?: string): Promise<RuntimePolicy> {
  return req(`/api/runtime/policy${modelId ? `?model_id=${encodeURIComponent(modelId)}` : ''}`);
}

/** What a code session's agent may do without asking (Claude Code's modes):
 *  ask before edits, accept edits, plan (read-only), auto. */
export type PermissionMode = 'ask' | 'accept_edits' | 'plan' | 'auto';

export async function getPermissionMode(): Promise<{ mode: PermissionMode }> {
  return req('/api/permissions/mode');
}

export async function setPermissionMode(mode: PermissionMode): Promise<{ mode: PermissionMode; resumed: number }> {
  return req('/api/permissions/mode', {
    method: 'PUT',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ mode }),
  });
}

export interface MetricsData {
  samples: any[];
  latest: any | null;
  alerts: { level: string; title: string; detail: string; suggestions: string[] }[];
  /** Name of the measured graphics card, when the driver reports one. */
  gpu_name?: string | null;
}

export async function getMetrics(window: string, signal?: AbortSignal): Promise<MetricsData> {
  return req(`/api/system/metrics?window=${encodeURIComponent(window)}`, { signal });
}

export async function getOverview(signal?: AbortSignal): Promise<any> {
  return req('/api/system/overview', { signal });
}

export interface Recommendation {
  recommended_ctx: number;
  profile: string;
  confidence: string;
  weights_gb: number;
  kv_gb: number;
  headroom_gb: number;
  rationale: string[];
  candidates: { ctx: number; kv_gb: number; total_gb: number; verdict: string }[];
}

export async function recommendContext(id: string, workload: string, profile: string): Promise<Recommendation> {
  return req(`/api/models/${id}/recommend?workload=${encodeURIComponent(workload)}&profile=${encodeURIComponent(profile)}`);
}

export interface CompatInfo {
  model: string;
  text: boolean;
  code: boolean;
  memory: boolean;
  tool_history: boolean;
  vision: boolean;
  tool_calling: boolean;
  warnings: string[];
}

export async function checkCompatibility(convId: string, modelId: string): Promise<CompatInfo> {
  return req(`/api/conversations/${convId}/compatibility?model_id=${encodeURIComponent(modelId)}`);
}

export interface PrepStage {
  stage: string;
  status: string;
  detail: string;
}

/** Prepare-context flow: staged SSE until done {ready, model}. */
export async function prepareContext(
  convId: string,
  modelId: string,
  onStage: (s: PrepStage) => void,
  signal: AbortSignal,
): Promise<{ ready: boolean; model: string }> {
  const r = await fetch(`/api/conversations/${convId}/prepare`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ model_id: modelId }),
    signal,
  });
  if (!r.ok || !r.body) throw new Error(`prepare failed: ${r.status}`);
  const reader = r.body.getReader();
  const dec = new TextDecoder();
  let buf = '';
  let result = { ready: false, model: modelId };
  const handle = (frame: string) => {
    let event = '';
    let data = '';
    for (const line of frame.split('\n')) {
      if (line.startsWith('event:')) event = line.slice(6).trim();
      else if (line.startsWith('data:')) {
        let v = line.slice(5);
        if (v.startsWith(' ')) v = v.slice(1);
        data += v;
      }
    }
    if (event === 'stage' && data) {
      try {
        onStage(JSON.parse(data));
      } catch { /* ignore */ }
    } else if (event === 'done' && data) {
      try {
        result = JSON.parse(data);
      } catch { /* ignore */ }
    } else if (event === 'error') {
      throw new Error(data || 'preparation failed');
    }
  };
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    buf += dec.decode(value, { stream: true });
    let idx: number;
    while ((idx = buf.indexOf('\n\n')) >= 0) {
      handle(buf.slice(0, idx));
      buf = buf.slice(idx + 2);
    }
  }
  return result;
}

export interface ArtifactInfo {
  id: string;
  conversation_id: string;
  filename: string;
  mime: string;
  size_bytes: number;
  created_at: string;
}

export async function listArtifacts(conversationId: string): Promise<ArtifactInfo[]> {
  return req(`/api/artifacts?conversation_id=${encodeURIComponent(conversationId)}`);
}

export function artifactUrl(id: string): string {
  return `/api/artifacts/${id}/file`;
}

export interface AgentEvent {
  context_usage?: AgentContextUsage;
  state: string;
  message: string;
  iteration: number;
  /** `thought_delta` is live partial model text: streamed only, never journaled or replayed. */
  kind?: 'task' | 'thought' | 'thought_delta' | 'tool_started' | 'tool_result' | 'permission' | 'final' | 'status' | string;
  tool?: string | null;
  args?: Record<string, unknown> | null;
  output?: string | null;
  diff?: string | null;
  pending_tool?: { tool: string; args: unknown; reason: string; session_grantable?: boolean } | null;
}

export type AgentStartResult =
  | { disposition?: 'run'; run_id: string; state?: string }
  | { disposition: 'conversation' | 'needs_task'; message: string; message_id: string };

export async function startAgent(workspace: string, task: string, mode: string, conversation_id?: string, options: {search?: boolean; reasoning?: boolean} = {}): Promise<AgentStartResult> {
  return req('/api/agent/run', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({
      workspace, task, mode, conversation_id: conversation_id ?? '', search: options.search ?? false,
      ...(options.reasoning !== undefined ? { reasoning: options.reasoning } : {}),
    }),
  });
}

export interface AgentRunSummary {
  id: string;
  /** Immutable backend run start; optional only for older backends. */
  started_at?: string;
  task: string;
  state: string;
  iterations: number;
  conversation_id: string;
  /** plan runs end with a plan the user approves; optional for older backends. */
  mode?: string;
  /** The run ended by presenting a plan to approve. */
  plan_ready?: boolean;
}

export async function agentRuns(): Promise<AgentRunSummary[]> {
  return req('/api/agent/runs');
}

export async function resumeAgent(runId: string, approved: boolean, grant_session = false) {
  return req(`/api/agent/runs/${runId}/resume`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ approved, grant_session }),
  });
}

export async function stopAgent(runId: string) {
  return req(`/api/agent/runs/${runId}/stop`, { method: 'POST' });
}

/** Tail a run's `event: agent` frames (replay + live) until terminal. */
export async function streamAgentEvents(
  runId: string,
  onEvent: (e: AgentEvent) => void,
  signal: AbortSignal,
): Promise<void> {
  const r = await fetch(`/api/agent/runs/${runId}/events`, { signal });
  // `status` 404: the server no longer has the run (runs live in memory, so a
  // restart forgets them); a view that remembered its id can drop it quietly.
  if (!r.ok || !r.body) throw Object.assign(new Error(`agent stream failed: ${r.status}`), { status: r.status });
  const reader = r.body.getReader();
  const dec = new TextDecoder();
  let buf = '';
  const handleFrame = (frame: string) => {
    let data = '';
    for (const line of frame.split('\n')) {
      if (line.startsWith('data:')) {
        let v = line.slice(5);
        if (v.startsWith(' ')) v = v.slice(1);
        data += (data ? '\n' : '') + v;
      }
    }
    if (data) {
      try {
        onEvent(JSON.parse(data));
      } catch { /* ignore malformed frames */ }
    }
  };
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    buf += dec.decode(value, { stream: true });
    let idx: number;
    while ((idx = buf.indexOf('\n\n')) >= 0) {
      handleFrame(buf.slice(0, idx));
      buf = buf.slice(idx + 2);
    }
  }
}

// ---- Stages 21–30 ----

export interface LoadProgress {
  model_id: string;
  stage: string;
  detail: string;
  updated_at: string;
  cancel_requested: boolean;
}

export async function getLoadProgress(): Promise<LoadProgress> {
  return req('/api/models/load/progress');
}

export async function cancelLoad() {
  return req('/api/models/load/cancel', { method: 'POST' });
}

export async function v1Status() {
  return req('/api/v1/status');
}

export interface MemoryEntry {
  id: string;
  scope_id: string;
  scope: string;
  content: string;
  source: string;
  created_at: string;
  last_used: string;
}

export async function listMemory(conversationId = '', workspaceId = ''): Promise<MemoryEntry[]> {
  return req(`/api/memory?conversation_id=${encodeURIComponent(conversationId)}&workspace_id=${encodeURIComponent(workspaceId)}`);
}

export async function addMemory(content: string, scope: string, scopeId: string, source = 'user'): Promise<MemoryEntry> {
  return req('/api/memory', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ content, scope, scope_id: scopeId, source }),
  });
}

export async function deleteMemory(id: string) {
  return req(`/api/memory/${id}`, { method: 'DELETE' });
}

export async function shareMemory(id: string, targetId: string, scope: string) {
  return req(`/api/memory/${id}/share`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ target_id: targetId, scope }),
  });
}

export interface TimelineEvent {
  kind: string;
  label: string;
  at: string;
}

export async function getTimeline(convId: string): Promise<{ conversation_id: string; events: TimelineEvent[] }> {
  return req(`/api/conversations/${convId}/timeline`);
}

export interface CompactStats {
  text: string;
  before_chars: number;
  after_chars: number;
  saved_pct: number;
  messages: number;
  status: string;
}

export async function compactConversation(convId: string): Promise<CompactStats> {
  return req(`/api/conversations/${convId}/compact`, { method: 'POST' });
}

export interface InstructionFile {
  file: string;
  chars: number;
  excerpt: string;
  scope: string;
}

export async function getInstructions(wsId: string): Promise<{ workspace: string; instructions: InstructionFile[] }> {
  return req(`/api/workspaces/${wsId}/instructions`);
}

export interface DiffInfo {
  workspace: string;
  stat: string;
  diff: string;
  truncated: boolean;
}

export async function getWorkspaceDiff(wsId: string): Promise<DiffInfo> {
  return req(`/api/workspaces/${wsId}/diff`);
}

export async function patchSession(id: string, patch: { priority?: string; related_to?: string }) {
  return req(`/api/sessions/${id}`, {
    method: 'PATCH',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(patch),
  });
}

export async function exportConversation(id: string) {
  return req(`/api/conversations/${id}/export`);
}

export interface RecoveryInfo {
  busy: string[];
  stale: { id: string; title: string; last_model: string; loaded_model: string; actions: string[] }[];
  note: string;
}

export async function getRecovery(): Promise<RecoveryInfo> {
  return req('/api/sessions/recovery');
}

export async function discardStale(id: string, loadedModel: string) {
  return patchConversation(id, { last_model: loadedModel } as never);
}

export async function sessionAction(id: string, action: 'pause' | 'stop' | 'reduce') {
  return req(`/api/sessions/${id}/action`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ action }),
  });
}

export interface AttachmentBudget {
  count: number;
  bytes: number;
  images: number;
  partial: number;
  unsupported: number;
  vision_model: boolean;
  warnings: string[];
}

export async function getAttachmentBudget(convId: string): Promise<AttachmentBudget> {
  return req(`/api/conversations/${convId}/attachment-budget`);
}

export async function ocrStatus(): Promise<{ available: boolean; detail: string }> {
  return req('/api/system/ocr');
}

export interface ArtifactDetail {
  id: string;
  filename: string;
  mime: string;
  size_bytes: number;
  path: string;
  openable_in_browser: boolean;
  url: string;
}

export async function artifactInfo(id: string): Promise<ArtifactDetail> {
  return req(`/api/artifacts/${id}/info`);
}

export async function cacheBudget(signal?: AbortSignal) {
  return req('/api/system/cache', { signal });
}

export async function deviceProfile() {
  return req('/api/system/device');
}

export async function capabilityDb() {
  return req('/api/system/capabilities');
}

export async function optimizeModel(id: string, workload = 'chat', policy = 'balanced') {
  return req(`/api/models/${id}/optimize?workload=${encodeURIComponent(workload)}&policy=${encodeURIComponent(policy)}`);
}

export async function deleteConversation(id: string) {
  const r = await fetch(`/api/conversations/${id}`, { method: 'DELETE' });
  if (!r.ok) throw new Error(`delete failed: ${r.status}`);
  return r.json();
}

// ---- Stages 31–38 ----

export interface RepoIndexInfo {
  workspace: string;
  files_total: number;
  symbols_total: number;
  truncated: boolean;
  query?: string;
  matches?: { path: string; size: number; ext: string; symbols: string[] }[];
  files?: { path: string; size: number; ext: string; symbols: string[] }[];
}

export async function getRepoIndex(wsId: string, q = '', refresh = false): Promise<RepoIndexInfo> {
  const p = new URLSearchParams();
  if (q) p.set('q', q);
  if (refresh) p.set('refresh', '1');
  const qs = p.toString();
  return req(`/api/workspaces/${wsId}/index${qs ? `?${qs}` : ''}`);
}

export interface KnowledgeInfo {
  paths: { path: string; chunks: number }[];
  total_chunks: number;
}

export async function listKnowledge(wsId: string): Promise<KnowledgeInfo> {
  return req(`/api/workspaces/${wsId}/knowledge`);
}

export async function ingestKnowledge(wsId: string, path: string) {
  return req(`/api/workspaces/${wsId}/knowledge`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ path }),
  });
}

export async function clearKnowledge(wsId: string, path = '') {
  return req(`/api/workspaces/${wsId}/knowledge/clear`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ path }),
  });
}

export interface PersistedMetric {
  timing?: OutputTiming | null;
  message_id: string;
  model_id: string;
  prompt_tokens: number;
  generated_tokens: number;
  gen_ms: number;
  ttft_ms: number;
  gen_tps: number;
  excerpt: string;
}

export async function getConversationMetrics(convId: string): Promise<{ metrics: PersistedMetric[] }> {
  return req(`/api/conversations/${convId}/metrics`);
}

export interface GitInfo {
  workspace: string;
  git: boolean;
  detail?: string;
  branch?: string;
  status?: string;
  log?: string;
}

export async function getWorkspaceGit(wsId: string): Promise<GitInfo> {
  return req(`/api/workspaces/${wsId}/git`);
}

export interface ToolDescriptor {
  name: string;
  description: string;
  risk: string;
  permission_required: string;
}

export async function listTools(): Promise<ToolDescriptor[]> {
  return req('/api/tools');
}

export interface ToolExecution {
  id: string;
  conversation_id: string;
  tool: string;
  args: string;
  result: string;
  approved: boolean;
  created_at: string;
}

export async function listToolExecutions(conversationId: string, limit = 20): Promise<ToolExecution[]> {
  return req(`/api/tools/executions?conversation_id=${encodeURIComponent(conversationId)}&limit=${limit}`);
}

export async function executeTool(workspace: string, tool: string, args: unknown, approvedOnce = false, conversationId = '') {
  return req('/api/tools/execute', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ workspace, tool, args, approved_once: approvedOnce, conversation_id: conversationId }),
  });
}

export interface PluginInfo {
  id: string;
  enabled: boolean;
  manifest?: { name?: string; version?: string; description?: string; tools?: { name: string; risk: string }[] };
  unknown_tools?: string[];
  error?: string;
}

export async function listPlugins(): Promise<{ plugins: PluginInfo[] }> {
  return req('/api/plugins');
}

export async function runPluginTool(pluginId: string, workspaceId: string, tool: string, args: unknown, approved = false) {
  return req(`/api/plugins/${pluginId}/run`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ workspace_id: workspaceId, tool, args, approved }),
  });
}

export interface DoctorCheck {
  id: string;
  label: string;
  status: 'ok' | 'warn' | 'fail';
  detail: string;
}

export async function getDoctor(): Promise<{ status: string; checks: DoctorCheck[] }> {
  return req('/api/doctor');
}

export interface SetupStatus {
  models_registered: number;
  models_with_gguf: number;
  binary_found: boolean;
  inference_running: boolean;
  current_model: string | null;
  conversations: number;
  steps: { id: string; label: string; done: boolean }[];
  needs_setup: boolean;
}

export async function getSetupStatus(): Promise<SetupStatus> {
  return req('/api/setup/status');
}

export interface BenchmarkResult {
  model: string;
  prompt_tokens: number;
  generated_tokens: number;
  total_ms: number;
  /** Engine-measured decode rate when reported, else wall-clock. */
  generation_tps: number;
  prompt_tps?: number | null;
  cached_tokens?: number | null;
  context_limit: number;
  sample_chars: number;
}

export async function runBenchmark(prompt = '', maxTokens = 64): Promise<BenchmarkResult> {
  return req('/api/system/benchmark', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ prompt, max_tokens: maxTokens }),
  });
}
