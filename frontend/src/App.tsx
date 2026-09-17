import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import MessageView from './components/MessageView';
import PrepareBanner from './components/PrepareBanner';
import ResourcesPanel from './components/ResourcesPanel';
import SettingsPanel from './components/SettingsPanel';
import Toasts, { pushToast, type Toast } from './components/Toasts';
import { ContextGauge } from './components/ContextBar';
import DiffModal from './components/DiffModal';
import ShareDialog, { type ShareOptions } from './components/ShareDialog';
import RecoveryBanner from './components/RecoveryBanner';
import ProjectActions, { useWorkspaceBranch } from './components/CodeHeader';
import AttachChips from './components/AttachChips';
import StartScripts from './components/StartScripts';
import SessionMenu from './components/SessionMenu';
import PermissionsModal from './components/PermissionsModal';
import RightPanel from './components/RightPanel';
import RightPanelTabs from './components/RightPanelTabs';
import ProjectLauncher from './components/ProjectLauncher';
import AgentChatProgress from './components/AgentChatProgress';
import CommandPalette, { type QuickAction } from './components/CommandPalette';
import WorkStatus from './components/WorkStatus';
import Rig, { machineState, type MachineActivity } from './components/Rig';
import Welcome from './components/Welcome';
import ModelsPage from './components/ModelsPage';
import RuntimePage from './components/RuntimePage';
import ToolsPage from './components/ToolsPage';
import { PERMISSION_MODE_DESCRIPTIONS, PERMISSION_MODE_LABELS, PROJECT_BOUNDARY_DESCRIPTION, SEARCH_PERMISSION_DESCRIPTION } from './components/permissionCopy';
import { VisibleOutputMeter, type GenerationPhase, type OutputTiming } from './services/outputTiming';
import { applyAgentContext } from './services/contextUsage';
import { currentActivitySnapshot, parseActivityStart, visibleWorkActivity } from './services/workElapsed';
import { APPROVE_PLAN_MESSAGE, autoTitle, matchesShortcut, nextPermissionMode, PERMISSION_MODE_SETTLE_MS, PERMISSION_MODES, PermissionModeSaver, selectAvailableModel, shouldStartAgent, updateMessage, WORKBENCH_DESTINATIONS } from './services/workbench';
import { Button, Dialog, IconButton, Kbd, Lamp, Notice, PopDivider, PopItem, PopLabel, Popover, Toggle } from './ui/primitives';
import { Icon, type IconName } from './ui/Icon';
import {
  agentRuns, compactConversation, createConversation, deleteConversation, deleteModel, discardStale, editMessage, exportConversation, forkConversation,
  getContext, getConversationMetrics, getMessages, getRecovery, getPermissionMode, getSettings, inferenceStart,
  inferenceStatus, listCommands, listConversations, listDownloads,
  listModels, listSessions, listTools, listWorkspaces, patchSession, stopAgent,
  loadModel, patchConversation, shareConversation,
  setPermissionMode as updatePermissionMode, startAgent, stopChat, streamChat, unloadModels,
  systemInfo, uploadAttachment, modelDetail,
  type AgentEvent, type CommandItem, type ContextInfo, type Conversation, type DownloadInfo,
  type InferenceStatus, type ModelMeta, type PermissionMode, type RecoveryInfo, type SessionInfo,
  type StreamUsage, type PersistedMetric, type ToolDescriptor, type Workspace,
} from './services/api';

type Msg = { id: string; role: 'user' | 'assistant' | 'tool'; text: string; time: string; activities?: AgentEvent[] };

type Theme = 'dark' | 'light' | 'system';
type PageId = 'models' | 'resources' | 'system' | 'tools' | 'settings';
type Perf = { tps: number | null; timing?: OutputTiming | null; legacy: boolean; model?: string };

const PAGE_META: Record<PageId, { title: string; description: string; icon: IconName }> = {
  models: { title: 'Models', description: 'Load, inspect and add GGUF models on this PC', icon: 'layers' },
  resources: { title: 'Resources', description: 'Live processor, memory and graphics readings for the whole machine', icon: 'activity' },
  system: { title: 'Runtime & diagnostics', description: 'The inference runtime, health checks and speed', icon: 'gauge' },
  tools: { title: 'Tools & plugins', description: 'Every action Companion can take and the approval it needs', icon: 'terminal' },
  settings: { title: 'Settings', description: 'Appearance, assistant behaviour, search and performance', icon: 'sliders' },
};

const PRIORITIES = [
  { id: 'background', label: 'Background' },
  { id: 'normal', label: 'Normal' },
  { id: 'high', label: 'High' },
] as const;

function applyTheme(t: Theme) {
  const mq = matchMedia('(prefers-color-scheme: light)');
  document.documentElement.dataset.theme = t === 'system' ? (mq.matches ? 'light' : 'dark') : t;
}

/** Keep the previous value when a poll returns identical data, so background
 *  refreshes do not re-render the whole conversation for nothing. */
function same<T>(next: T) {
  return (previous: T) => JSON.stringify(previous) === JSON.stringify(next) ? previous : next;
}

function shortcutLabel(binding: string) {
  return binding.split('+').map((part) => part.trim()).map((part) => part.length === 1 ? part.toUpperCase() : part[0].toUpperCase() + part.slice(1)).join(' ');
}

export default function App() {
  const [tab, setTab] = useState<'chat' | PageId>('chat');
  const [mode, setMode] = useState<'chat' | 'code'>(() => (localStorage.getItem('companion.mode') as any) || 'chat');
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [paletteShortcut, setPaletteShortcut] = useState('ctrl+k');
  const [sessionFilter, setSessionFilter] = useState('');
  const [theme, setTheme] = useState<Theme>(() => (localStorage.getItem('companion.theme') as Theme) || 'dark');
  const [models, setModels] = useState<ModelMeta[]>([]);
  const [modelId, setModelId] = useState('');
  const preferredDefaultModel = useRef('');
  const modelDefaultsRead = useRef(false);
  const [reasoningCapable, setReasoningCapable] = useState(false);
  const [convs, setConvs] = useState<Conversation[]>([]);
  const convsRef = useRef(convs);
  convsRef.current = convs;
  const [convId, setConvId] = useState<string | null>(null);
  const [msgs, setMsgs] = useState<Msg[]>([]);
  const [historyLoading, setHistoryLoading] = useState(false);
  const [msgLimit, setMsgLimit] = useState(150);
  const [input, setInput] = useState('');
  const [attachTick, setAttachTick] = useState(0);
  const [busy, setBusy] = useState(false);
  const [chatActivity, setChatActivity] = useState<{ conversationId: string; startedAt: number } | null>(null);
  const [statusLine, setStatusLine] = useState('');
  const [sys, setSys] = useState<any>(null);
  const [inf, setInf] = useState<InferenceStatus | null>(null);
  const [usage, setUsage] = useState<StreamUsage | null>(null);
  const [downloads, setDownloads] = useState<DownloadInfo[]>([]);
  const [toasts, setToasts] = useState<Toast[]>([]);
  const [ctx, setCtx] = useState<ContextInfo | null>(null);
  const [editing, setEditing] = useState<{ mid: string; draft: string } | null>(null);
  const [reasoning, setReasoning] = useState(false);
  const [search, setSearch] = useState(false);
  const [cmdMenu, setCmdMenu] = useState<CommandItem[]>([]);
  const [cmdSel, setCmdSel] = useState(0);
  const [workspaces, setWorkspaces] = useState<Workspace[]>([]);
  const [wsId, setWsId] = useState(() => localStorage.getItem('companion.workspace') ?? '');
  const [projectLauncherOpen, setProjectLauncherOpen] = useState(false);
  const [projectMenuOpen, setProjectMenuOpen] = useState(false);
  const [showShare, setShowShare] = useState(false);
  const [focusRun, setFocusRun] = useState<string | null>(null);
  const [guard, setGuard] = useState<{ kind: 'load' | 'start'; id: string; detail: string } | null>(null);
  const [confirmState, setConfirmState] = useState<{ title: string; body: string; action: string; icon?: IconName; onConfirm: () => void } | null>(null);
  const [sessions, setSessions] = useState<SessionInfo[]>([]);
  const [collapsed, setCollapsed] = useState(() => typeof window !== 'undefined' && window.matchMedia('(max-width: 860px)').matches);
  const [narrow, setNarrow] = useState(() => typeof window !== 'undefined' && window.matchMedia('(max-width: 860px)').matches);
  const [mobileNav, setMobileNav] = useState(false);
  const [showLatest, setShowLatest] = useState(false);
  const [backendUp, setBackendUp] = useState<boolean | null>(null);
  const [rightOpen, setRightOpen] = useState(false);
  const [rightTab, setRightTab] = useState(mode === 'code' ? 'activity' : 'context');
  const [pinnedIds, setPinnedIds] = useState<string[]>(() => {
    try { return JSON.parse(localStorage.getItem('companion.pinned') ?? '[]'); } catch { return []; }
  });
  const [renaming, setRenaming] = useState<{ id: string; draft: string; where: 'list' | 'head' } | null>(null);
  const [sessionMenuOpen, setSessionMenuOpen] = useState(false);
  const [permOpen, setPermOpen] = useState(false);
  const [registry, setRegistry] = useState<ToolDescriptor[]>([]);
  const [loadingModel, setLoadingModel] = useState(false);
  const [agentBusy, setAgentBusy] = useState(false);
  const [agentActivity, setAgentActivity] = useState<{ conversationId: string; runId: string; startedAt: number | null } | null>(null);
  const pendingAgentStarts = useRef(new Set<string>());
  const refreshAgentActivity = useRef<() => void>(() => {});
  const [agentPhase, setAgentPhase] = useState('PLANNING');
  // The persisted backend policy is authoritative; browser storage is not a grant.
  const [permissionMode, setPermissionModeState] = useState<PermissionMode>('ask');
  // True until the saved mode is read; saves themselves never lock the picker.
  const [permissionModeBusy, setPermissionModeBusy] = useState(true);
  const modeSaver = useRef<PermissionModeSaver<PermissionMode, { mode: PermissionMode; resumed: number }>>(null as never);
  if (!modeSaver.current) {
    modeSaver.current = new PermissionModeSaver<PermissionMode, { mode: PermissionMode; resumed: number }>('ask', updatePermissionMode, ({ mode: saved, result, error }) => {
      setPermissionModeState(saved);
      if (error !== undefined) {
        notify('error', (error as any)?.message ?? 'Could not change permission mode.');
        return;
      }
      localStorage.setItem('companion.permissionMode', saved);
      const resumed = result?.resumed ? ` — resumed ${result.resumed} waiting task${result.resumed === 1 ? '' : 's'}` : '';
      notify('success', `${PERMISSION_MODE_LABELS[saved]} mode${resumed}. ${PERMISSION_MODE_DESCRIPTIONS[saved]}`);
    });
  }
  const [compacting, setCompacting] = useState(false);
  const [diffWs, setDiffWs] = useState<string | null>(null);
  const [recovery, setRecovery] = useState<RecoveryInfo | null>(null);
  const [recoveryOff, setRecoveryOff] = useState(false);
  const [prepareDismissed, setPrepareDismissed] = useState<Record<string, boolean>>({});
  const [dragOver, setDragOver] = useState(false);
  const [liveTps, setLiveTps] = useState<number | null>(null);
  const [showGenerationSpeed, setShowGenerationSpeed] = useState(true);
  const [showDetailedMetrics, setShowDetailedMetrics] = useState(false);
  const [perfMap, setPerfMap] = useState<Record<string, Perf>>({});
  // Native reasoning streamed this session, keyed by reply id. Never persisted:
  // reloading the page shows the answer without its thought process.
  const [thinkingMap, setThinkingMap] = useState<Record<string, string>>({});
  const [generationPhase, setGenerationPhase] = useState<GenerationPhase>('processing');
  const lastTpsPush = useRef(0);
  const composerRef = useRef<HTMLTextAreaElement | null>(null);
  const abort = useRef<AbortController | null>(null);
  const fileRef = useRef<HTMLInputElement | null>(null);
  const conversationRef = useRef<string | null>(convId);
  conversationRef.current = convId;
  const transcriptRef = useRef<HTMLDivElement>(null);
  const centerRef = useRef<HTMLDivElement>(null);
  const dockRef = useRef<HTMLDivElement>(null);
  const followOutput = useRef(true);
  const drafts = useRef<Record<string, string>>({});

  const notify = useCallback((kind: Toast['kind'], text: string) => pushToast(setToasts, kind, text), []);
  const dismissToast = useCallback((id: number) => setToasts((items) => items.filter((toast) => toast.id !== id)), []);

  useEffect(() => {
    applyTheme(theme);
    localStorage.setItem('companion.theme', theme);
  }, [theme]);

  useEffect(() => {
    const mq = window.matchMedia('(max-width: 860px)');
    const onChange = () => { setNarrow(mq.matches); if (!mq.matches) setMobileNav(false); };
    mq.addEventListener('change', onChange);
    return () => mq.removeEventListener('change', onChange);
  }, []);

  useEffect(() => {
    const onAppearance = (event: Event) => {
      const value = (event as CustomEvent).detail;
      if (['dark', 'light', 'system'].includes(value?.theme)) setTheme(value.theme);
    };
    window.addEventListener('companion:appearance', onAppearance);
    const onSettings = (event: Event) => {
      const value = (event as CustomEvent).detail;
      const saved: PermissionMode = (PERMISSION_MODES as readonly string[]).includes(value.agent?.permission_mode) ? value.agent.permission_mode : value.agent?.autonomous_enabled ? 'auto' : 'ask';
      modeSaver.current.reset(saved);
      setPermissionModeState(saved);
      if (value.keyboard?.command_palette) setPaletteShortcut(value.keyboard.command_palette);
      preferredDefaultModel.current = value.general?.default_model ?? '';
      setShowGenerationSpeed(value.diagnostics?.show_generation_speed ?? true);
      setShowDetailedMetrics(value.diagnostics?.show_detailed_metrics ?? false);
    };
    window.addEventListener('companion:settings', onSettings);
    getSettings().then((settings) => {
      if (settings.keyboard?.command_palette) setPaletteShortcut(settings.keyboard.command_palette);
      setShowGenerationSpeed(settings.diagnostics?.show_generation_speed ?? true);
      setShowDetailedMetrics(settings.diagnostics?.show_detailed_metrics ?? false);
      document.documentElement.dataset.density = settings.appearance?.density ?? 'comfortable';
      document.documentElement.classList.toggle('reduce-motion', !!settings.appearance?.reduce_motion);
    }).catch(() => {});
    return () => { window.removeEventListener('companion:appearance', onAppearance); window.removeEventListener('companion:settings', onSettings); };
  }, []);

  useEffect(() => {
    const mq = matchMedia('(prefers-color-scheme: light)');
    const onChange = () => applyTheme((localStorage.getItem('companion.theme') as Theme) || 'dark');
    mq.addEventListener('change', onChange);
    return () => mq.removeEventListener('change', onChange);
  }, []);

  async function refreshModels() {
    try {
      const [next, initialSettings] = await Promise.all([listModels(), modelDefaultsRead.current ? Promise.resolve(null) : getSettings().catch(() => null)]);
      if (initialSettings) { preferredDefaultModel.current = initialSettings.general?.default_model ?? ''; modelDefaultsRead.current = true; }
      setModels(same(next));
      // A model can disappear between launches (for example, after its
      // directory is removed or the models folder is reset). Never keep a
      // stale id in the selector: it would make Load/Start send an id the
      // backend can no longer resolve.
      setModelId((current) => selectAvailableModel(next, current, preferredDefaultModel.current));
    } catch { /* backend offline */ }
  }

  async function refreshDownloads() {
    try {
      setDownloads(same(await listDownloads()));
    } catch { /* backend offline */ }
  }

  async function refreshConvs(select?: string) {
    try {
      const c = await listConversations();
      setConvs(c);
      if (select) void selectConv(select, c);
      else if (!convId || !c.some((conversation) => conversation.id === convId)) {
        const remembered = localStorage.getItem(`companion.last.${mode}`);
        const next = c.find((conversation) => conversation.id === remembered && (conversation.mode || 'chat') === mode)
          ?? c.find((conversation) => (conversation.mode || 'chat') === mode);
        if (next) void selectConv(next.id, c);
      }
    } catch {
      setConvs([]);
    }
  }

  async function refreshSessions() {
    try {
      const r = await listSessions();
      setSessions(same(r.sessions));
    } catch { /* ignore */ }
  }

  async function refreshWorkspaces() {
    try {
      setWorkspaces(await listWorkspaces());
    } catch { /* ignore */ }
  }

  async function changeWorkspace(nextId: string) {
    if (busy || agentBusy) { notify('warning', 'Stop the current work before changing its project.'); return; }
    const active = convs.find((conversation) => conversation.id === convId);
    if (active?.mode === 'code' && active.workspace !== nextId) {
      if (!nextId) { notify('warning', 'A code session must stay linked to a project.'); return; }
      try {
        const updated = await patchConversation(active.id, { workspace: nextId }) as Conversation;
        setConvs((items) => items.map((conversation) => conversation.id === updated.id ? updated : conversation));
        notify('success', `Linked this session to ${workspaces.find((workspace) => workspace.id === nextId)?.name ?? 'the selected project'}.`);
      } catch (error: any) {
        notify('error', error?.message ?? 'Could not link the project.');
        return;
      }
    }
    setWsId(nextId);
    localStorage.setItem('companion.workspace', nextId);
  }

  async function refreshRegistry() {
    try {
      setRegistry(await listTools());
    } catch { /* ignore */ }
  }

  useEffect(() => {
    refreshModels();
    refreshDownloads();
    refreshConvs();
    refreshWorkspaces();
    refreshRegistry();
    refreshSessions();
    getRecovery().then(setRecovery).catch(() => setRecovery(null));
    const t = setInterval(refreshDownloads, 2000);
    const modelRefresh = setInterval(() => { if (!document.hidden) { void refreshModels(); inferenceStatus().then((state) => { setInf(same<InferenceStatus | null>(state)); setBackendUp(true); }).catch(() => setBackendUp(false)); } }, 10000);
    // Session activity drives the live lamps in the list; keep it fresh but cheap.
    const sessionRefresh = setInterval(() => { if (!document.hidden) void refreshSessions(); }, 6000);
    systemInfo().then((v) => { setSys(v); setBackendUp(true); }).catch(() => { setSys(null); setBackendUp(false); });
    inferenceStatus().then(setInf).catch(() => setInf(null));
    getPermissionMode().then((result) => {
      modeSaver.current.reset(result.mode);
      setPermissionModeState(result.mode);
      localStorage.setItem('companion.permissionMode', result.mode);
    }).catch(() => notify('warning', 'Could not read the saved approval policy. Reconnect to the local runtime before changing it.'))
      .finally(() => setPermissionModeBusy(false));
    return () => { clearInterval(t); clearInterval(modelRefresh); clearInterval(sessionRefresh); };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  /** Returns whether the mode is now `next`. The picker shows it at once. */
  function changePermissionMode(next: PermissionMode): Promise<boolean> {
    setPermissionModeState(next);
    return modeSaver.current.request(next);
  }

  /** Approving a plan switches the mode first, then carries the plan out.
   *  True once the work started; otherwise the reason was shown. */
  async function approvePlan(next: 'accept_edits' | 'ask'): Promise<boolean> {
    if (busy || agentBusy) {
      notify('warning', 'Wait for the current work to finish, then approve the plan.');
      return false;
    }
    if (!inf?.running || models.find((model) => model.loaded)?.id !== modelId) {
      notify('warning', 'No model is loaded. Load one from the panel at the bottom left, then approve the plan.');
      return false;
    }
    if (!(await changePermissionMode(next))) return false;
    return send(APPROVE_PLAN_MESSAGE, next);
  }

  /** "No, keep planning": the next message plans too. */
  async function keepPlanning(): Promise<boolean> {
    const kept = await changePermissionMode('plan');
    composerRef.current?.focus();
    return kept;
  }

  // Workspace IDs are security boundaries. A missing one requires an explicit
  // project selection, even when only one other workspace exists.

  useEffect(() => {
    localStorage.setItem('companion.mode', mode);
  }, [mode]);

  // The two primary destinations remain keyboard-addressable.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const mod = e.ctrlKey || e.metaKey;
      if (mod && e.key === '1') { e.preventDefault(); switchMode('chat'); }
      else if (mod && e.key === '2') { e.preventDefault(); switchMode('code'); }
      else if (matchesShortcut(e, paletteShortcut) || (e.metaKey && e.key.toLowerCase() === 'k' && paletteShortcut === 'ctrl+k')) { e.preventDefault(); setPaletteOpen((open) => !open); }
      else if (e.key === 'Escape') {
        setCmdMenu([]);
        setShowShare(false);
        setDiffWs(null);
        setProjectLauncherOpen(false);
        setPermOpen(false);
        setMobileNav(false);
        setPaletteOpen(false);
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [convs, busy, agentBusy, convId, input, paletteShortcut]);

  useEffect(() => {
    const view = transcriptRef.current;
    if (view && followOutput.current) view.scrollTop = view.scrollHeight;
  }, [msgs, agentBusy]);

  // One notification region for every view: it floats just above the dock
  // (composer, status, notices) and follows it as the dock grows or shrinks.
  useEffect(() => {
    const center = centerRef.current;
    const dock = dockRef.current;
    if (!center) return;
    if (!dock) { center.style.setProperty('--toast-bottom', '24px'); return; }
    const update = () => center.style.setProperty('--toast-bottom', `${dock.offsetHeight + 8}px`);
    update();
    const observer = new ResizeObserver(update);
    observer.observe(dock);
    return () => observer.disconnect();
  }, [tab]);

  useEffect(() => {
    if (!modelId) {
      setReasoningCapable(false);
      return;
    }
    modelDetail(modelId).then((d) => setReasoningCapable(!!d.metadata.supports_reasoning)).catch(() => setReasoningCapable(false));
  }, [modelId]);

  // Keep the composer aligned with the active code session even when the
  // activity panel is closed while work continues in the background.
  useEffect(() => {
    if (mode !== 'code' || !convId) {
      setAgentBusy(false);
      setAgentActivity(null);
      return;
    }
    let mounted = true;
    let latestRequest = 0;
    const refresh = () => {
      const request = ++latestRequest;
      if (pendingAgentStarts.current.has(convId)) {
        setAgentBusy(true);
        setAgentActivity({ conversationId: convId, runId: '', startedAt: null });
        setAgentPhase('ROUTING');
        return;
      }
      void agentRuns().then((runs) => {
        if (!mounted || !currentActivitySnapshot(convId, conversationRef.current, request, latestRequest, pendingAgentStarts.current.has(convId))) return;
        const live = [...runs].reverse().find((run) => run.conversation_id === convId && !['COMPLETED', 'FAILED', 'CANCELLED'].includes(run.state));
        setAgentBusy(!!live);
        setAgentActivity(same(live ? { conversationId: convId, runId: live.id, startedAt: parseActivityStart(live.started_at) } : null));
        if (live) setAgentPhase(live.state);
        if (live) setFocusRun(live.id);
      }).catch(() => {});
    };
    refreshAgentActivity.current = refresh;
    void refresh();
    const timer = window.setInterval(refresh, 2000);
    return () => {
      mounted = false;
      window.clearInterval(timer);
      if (refreshAgentActivity.current === refresh) refreshAgentActivity.current = () => {};
    };
  }, [mode, convId]);

  // Metrics belong to a message ID, not a repeated answer or shared text prefix.
  function applyMetrics(_texts: string[], metrics: PersistedMetric[]) {
    if (!metrics.length) return;
    setPerfMap((p) => {
      const next = { ...p };
      for (const m of metrics) {
        next[m.message_id] = { tps: m.timing ? m.timing.output_tps : m.gen_tps, timing: m.timing, legacy: !m.timing, model: m.model_id };
      }
      return next;
    });
  }

  function mergeMetrics(id: string, texts: string[]) {
    getConversationMetrics(id)
      .then((r) => applyMetrics(texts, r.metrics))
      .catch(() => {});
  }

  async function selectConv(id: string, source = convs) {
    drafts.current[convId ?? 'new'] = input;
    setInput(drafts.current[id] ?? '');
    conversationRef.current = id;
    followOutput.current = true;
    setShowLatest(false);
    setConvId(id);
    setMsgs([]);
    setHistoryLoading(true);
    setUsage(null);
    setCtx(null);
    setEditing(null);
    setFocusRun(null);
    setMsgLimit(150);
    setMobileNav(false);
    try {
      const conv = source.find((c) => c.id === id);
      const nextMode = (conv?.mode || 'chat') as 'chat' | 'code';
      setMode(nextMode);
      if (nextMode !== mode) setRightTab(nextMode === 'code' ? 'activity' : 'context');
      setTab('chat');
      localStorage.setItem(`companion.last.${nextMode}`, id);
      if (conv?.mode === 'code' && conv.workspace) {
        setWsId(conv.workspace);
        localStorage.setItem('companion.workspace', conv.workspace);
      }
      const h = await getMessages(id);
      if (conversationRef.current !== id) return;
      setMsgs(h.map((m) => ({ id: m.id, role: m.role, text: m.content, time: m.created_at, activities: m.activities })));
      setReasoning(!!conv?.reasoning_default);
      setSearch(!!conv?.search_default);
      getContext(id).then((context) => { if (conversationRef.current === id) setCtx(context); }).catch(() => {});
      mergeMetrics(id, h.filter((m) => m.role === 'assistant').map((m) => m.content));
    } catch {
      if (conversationRef.current === id) setMsgs([]);
    } finally {
      if (conversationRef.current === id) setHistoryLoading(false);
    }
  }

  function switchMode(nextMode: 'chat' | 'code') {
    setMode(nextMode);
    setTab('chat');
    setMobileNav(false);
    setRightTab(nextMode === 'code' ? 'activity' : 'context');
    const remembered = localStorage.getItem(`companion.last.${nextMode}`);
    const next = convs.find((conversation) => conversation.id === remembered && (conversation.mode || 'chat') === nextMode)
      ?? convs.find((conversation) => (conversation.mode || 'chat') === nextMode);
    if (next) void selectConv(next.id);
    else {
      conversationRef.current = null;
      setConvId(null);
      setHistoryLoading(false);
      setInput(drafts.current['new'] ?? '');
      setMsgs([]);
      setCtx(null);
    }
  }

  async function reloadMsgs(cid: string) {
    try {
      const h = await getMessages(cid);
      if (conversationRef.current !== cid) return;
      setMsgs(h.map((m) => ({ id: m.id, role: m.role, text: m.content, time: m.created_at, activities: m.activities })));
      getContext(cid).then((context) => { if (conversationRef.current === cid) setCtx(context); }).catch(() => { if (conversationRef.current === cid) setCtx(null); });
      mergeMetrics(cid, h.filter((m) => m.role === 'assistant').map((m) => m.content));
    } catch { /* keep optimistic view */ }
  }

  async function newChat() {
    try {
      const extra = mode === 'code' ? { mode, workspace: wsId } : { mode };
      if (mode === 'code' && !workspaces.some((workspace) => workspace.id === wsId)) {
        notify('warning', 'Choose a project before starting a code session.');
        setProjectLauncherOpen(true);
        return;
      }
      const c = await createConversation(mode === 'code' ? 'New task' : 'New chat', modelId, extra);
      setConvs((prev) => [c, ...prev]);
      conversationRef.current = c.id;
      setConvId(c.id);
      setHistoryLoading(false);
      setFocusRun(null);
      setTab('chat');
      setMobileNav(false);
      setInput('');
      localStorage.setItem(`companion.last.${mode}`, c.id);
      setMsgs([]);
      setCtx(null);
      refreshSessions();
      requestAnimationFrame(() => composerRef.current?.focus());
    } catch (e: any) {
      notify('error', e?.message ?? 'Could not create conversation (is the backend running?)');
    }
  }

  /** `modeOverride`: the permission mode this message runs in when it was
   *  changed in the same step (approving a plan), before state has updated. */
  async function send(override?: string, modeOverride?: PermissionMode): Promise<boolean> {
    const raw = override ?? input;
    if (!raw.trim() || busy || agentBusy || (convId && pendingAgentStarts.current.has(convId))) return false;
    if (!raw.trim().startsWith('/') && (!inf?.running || models.find((model) => model.loaded)?.id !== modelId)) {
      notify('warning', 'No model is loaded. Load one from the panel at the bottom left — your draft is kept.');
      return false;
    }
    // A mode chosen with Shift+Tab a moment ago is in effect before work starts.
    if (mode === 'code' && !(await modeSaver.current.settle())) return false;
    let cid = convId;
    if (!cid) {
      try {
        const extra = mode === 'code' ? { mode, workspace: wsId } : { mode };
        if (mode === 'code' && !workspaces.some((workspace) => workspace.id === wsId)) {
          notify('warning', 'Choose a project first.');
          setProjectLauncherOpen(true);
          return false;
        }
        const c = await createConversation(raw.slice(0, 60) || 'New chat', modelId, extra);
        setConvs((prev) => [c, ...prev]);
        cid = c.id;
        conversationRef.current = cid;
        setConvId(cid);
        localStorage.setItem(`companion.last.${mode}`, cid);
      } catch (e: any) {
        setMsgs((m) => [...m, { id: `tmp-${Date.now()}`, role: 'tool', text: `Error: ${e?.message ?? e}`, time: '' }]);
        return false;
      }
    }
    const text = raw;
    if (override === undefined) {
      setInput('');
      if (composerRef.current) composerRef.current.style.height = 'auto';
    }
    setCmdMenu([]);
    const tmpId = `tmp-${Date.now()}`;
    setMsgs((m) => [...m, { id: tmpId, role: 'user', text, time: '' }]);

    followOutput.current = true;
    if (shouldStartAgent(mode, text)) {
      const linkedWorkspace = convs.find((conversation) => conversation.id === cid)?.workspace ?? wsId;
      const workspace = workspaces.find((candidate) => candidate.id === linkedWorkspace);
      if (!workspace) {
        setMsgs((messages) => messages.filter((message) => message.id !== tmpId));
        notify('warning', 'Choose a project so the agent has a safe working boundary.');
        setProjectLauncherOpen(true);
        return false;
      }
      setAgentBusy(true);
      pendingAgentStarts.current.add(cid);
      setAgentActivity({ conversationId: cid, runId: '', startedAt: null });
      setAgentPhase('ROUTING');
      let started = false;
      try {
        const agentMode = (modeOverride ?? modeSaver.current.mode) === 'plan' ? 'plan' : 'agent';
        const run = await startAgent(workspace.path, text.trim(), agentMode, cid, {search, reasoning});
        if ('run_id' in run) {
          started = true;
          // Progress and approvals show in the conversation, as in Claude
          // Code; the inspector stays as the user left it.
          if (conversationRef.current === cid) {
            setFocusRun(run.run_id);
            setAgentPhase('PLANNING');
          }
          void getContext(cid).then((context) => { if (conversationRef.current === cid) setCtx(context); }).catch(() => {});
          void maybeAutoTitle(cid, text);
        } else {
          if (conversationRef.current === cid) {
            setAgentBusy(false);
            setAgentActivity(null);
            setFocusRun(null);
            setMsgs((messages) => [...messages, { id: run.message_id, role: 'assistant', text: run.message, time: '' }]);
            composerRef.current?.focus();
          }
        }
      } catch (e: any) {
        if (conversationRef.current === cid) {
          setAgentBusy(false);
          setAgentActivity(null);
          setMsgs((messages) => [...messages, { id: `tmp-${Date.now()}`, role: 'tool', text: `Could not start work: ${e?.message ?? e}`, time: '' }]);
        }
        notify('error', e?.message ?? 'Could not start the code task.');
      } finally {
        pendingAgentStarts.current.delete(cid);
        if (conversationRef.current === cid) refreshAgentActivity.current();
        void reloadMsgs(cid);
        refreshSessions();
        // The run records the model now working on this session: the "Last used
        // with" notice must see it.
        void refreshConvs();
      }
      return started;
    }
    setBusy(true);
    setChatActivity({ conversationId: cid, startedAt: Date.now() });
    setStatusLine('');
    setLiveTps(null);
    setGenerationPhase('processing');
    const outputMeter = new VisibleOutputMeter();
    lastTpsPush.current = 0;
    const ctl = new AbortController();
    abort.current = ctl;
    let acc = '';
    setUsage(null);
    const answeringModel = models.find((model) => model.loaded)?.id;
    setMsgs((m) => [...m, { id: `${tmpId}-a`, role: 'assistant', text: '', time: '' }]);
    try {
      await streamChat(text, cid, {
        onToken: (tok) => {
          acc += tok;
          if (conversationRef.current !== cid) return;
          if (!tok) return;
          setGenerationPhase('responding');
          setStatusLine('');
          const now = performance.now();
          const rate = outputMeter.append(tok, now);
          if (now - lastTpsPush.current > 250) {
            lastTpsPush.current = now;
            setLiveTps(rate);
          }
          setMsgs((messages) => updateMessage(messages, `${tmpId}-a`, (message) => ({ ...message, text: acc })));
        },
        onAction: ({ state, name }) => {
          if (conversationRef.current !== cid) return;
          const label = name ? name.replace(/_/g, ' ') : 'an action';
          setStatusLine(state === 'started' ? `Preparing ${label}…` : state === 'incomplete' ? `The ${label} request was cut off.` : '');
        },
        onReplace: (text) => {
          acc = text;
          if (conversationRef.current !== cid) return;
          setMsgs((messages) => updateMessage(messages, `${tmpId}-a`, (message) => ({ ...message, text: acc })));
        },
        onReasoning: (t) => {
          if (conversationRef.current !== cid || !t) return;
          setGenerationPhase('thinking');
          setThinkingMap((map) => ({ ...map, [`${tmpId}-a`]: (map[`${tmpId}-a`] ?? '') + t }));
        },
        onStatus: (s) => { if (conversationRef.current === cid) setStatusLine(s); },
        onPhase: ({phase}) => {
          if (conversationRef.current !== cid) return;
          setGenerationPhase(phase);
          setStatusLine('');
          if (phase !== 'responding') { outputMeter.pause(); setLiveTps(null); }
        },
        onActivity: (event) => {
          if (conversationRef.current !== cid) return;
          setMsgs((messages) => updateMessage(messages, `${tmpId}-a`, (message) => {
            const activities = [...(message.activities ?? [])];
            const index = (event.kind === 'tool_result' || event.kind === 'tool_error') ? activities.findIndex((prior) => prior.kind === 'tool_started' && prior.iteration === event.iteration && prior.tool === event.tool) : -1;
            if (index >= 0) activities[index] = event; else activities.push(event);
            return { ...message, activities };
          }));
        },
        onDone: (u) => {
          if (conversationRef.current !== cid) return;
          setUsage(u);
          setStatusLine('');
          setLiveTps(null);
          if (u.timing || u.gen_tps != null) {
            const entry: Perf = { tps: u.timing ? (u.timing.engine_output_tps ?? u.timing.output_tps) : u.gen_tps ?? null, timing: u.timing, legacy: !u.timing, model: answeringModel };
            setPerfMap((p) => ({ ...p, [u.message_id ?? `${tmpId}-a`]: entry }));
          }
          if (u.message_id) {
            // The persisted reply id replaces the temporary streaming id.
            setThinkingMap((map) => {
              const thought = map[`${tmpId}-a`];
              if (!thought) return map;
              const { [`${tmpId}-a`]: _dropped, ...rest } = map;
              return { ...rest, [u.message_id!]: thought };
            });
          }
          const cmd = u.command;
          if (cmd?.type === 'clear' && cmd.conversation_id) {
            refreshConvs(cmd.conversation_id);
          } else if (cmd?.type === 'retry' && cmd.text) {
            void send(cmd.text);
          } else if (cmd?.type === 'agent' && cmd.run_id) {
            setFocusRun(cmd.run_id);
          }
          if (cid) void maybeAutoTitle(cid, text);
        },
        onError: (msg) => { if (conversationRef.current === cid) setMsgs((m) => [...m, { id: `tmp-${Date.now()}`, role: 'tool', text: `Error: ${msg}`, time: '' }]); },
      }, ctl.signal, { reasoning, search });
    } catch (e: any) {
      if (e?.name !== 'AbortError' && conversationRef.current === cid) {
        setMsgs((m) => [...m, { id: `tmp-${Date.now()}`, role: 'tool', text: `Error: ${e?.message ?? e}`, time: '' }]);
      }
    } finally {
      setBusy(false);
      setChatActivity(null);
      setStatusLine('');
      void reloadMsgs(cid);
      refreshSessions();
      // A reply records the model that answered; without reloading the list the
      // "Last used with another model" notice stayed after replying.
      void refreshConvs();
    }
    return true;
  }

  function regenerate() {
    if (busy || msgs.length === 0) return;
    const lastUser = [...msgs].reverse().find((m) => m.role === 'user');
    if (!lastUser) return;
    setMsgs((m) => {
      const c = [...m];
      while (c.length > 0 && c[c.length - 1].role !== 'user') c.pop();
      return c;
    });
    void send(lastUser.text);
  }

  async function saveEdit() {
    if (!convId || !editing || !editing.draft.trim() || busy) return;
    try {
      const r = await editMessage(convId, editing.mid, editing.draft.trim());
      setEditing(null);
      notify('info', `Edited. ${r.truncated} later message(s) removed — send a message to continue from there.`);
      await reloadMsgs(convId);
    } catch (e: any) {
      notify('error', e?.message ?? 'Edit failed.');
    }
  }

  async function attach(f: File | undefined) {
    if (!f || !convId) {
      if (!convId) notify('warning', 'Start a chat first, then attach files.');
      return;
    }
    try {
      const a = await uploadAttachment(convId, f);
      notify('success', `Attached ${a.filename} (${(a.size_bytes / 1024).toFixed(1)} KB) — its text is now in context.`);
      setAttachTick((t) => t + 1);
      getContext(convId).then(setCtx).catch(() => {});
    } catch (e: any) {
      notify('error', e?.message ?? 'Attach failed.');
    }
  }

  async function onInput(v: string) {
    setInput(v);
    if (/^\/[a-z-]*$/i.test(v)) {
      try {
        setCmdMenu(await listCommands(v));
        setCmdSel(0);
      } catch {
        setCmdMenu([]);
      }
    } else {
      setCmdMenu([]);
    }
  }

  function composerKey(e: React.KeyboardEvent) {
    if (cmdMenu.length > 0) {
      if (e.key === 'ArrowDown') { e.preventDefault(); setCmdSel((s) => (s + 1) % cmdMenu.length); return; }
      if (e.key === 'ArrowUp') { e.preventDefault(); setCmdSel((s) => (s - 1 + cmdMenu.length) % cmdMenu.length); return; }
      if (e.key === 'Tab' || e.key === 'Enter') {
        e.preventDefault();
        setInput(`/${cmdMenu[cmdSel].name} `);
        setCmdMenu([]);
        return;
      }
      if (e.key === 'Escape') { setCmdMenu([]); return; }
    }
    // Shift+Tab cycles the permission mode, as in Claude Code.
    if (mode === 'code' && e.key === 'Tab' && e.shiftKey && !e.ctrlKey && !e.altKey && !e.metaKey) {
      e.preventDefault();
      if (!busy && !agentBusy && !permissionModeBusy) {
        const next = nextPermissionMode(permissionMode);
        setPermissionModeState(next);
        modeSaver.current.schedule(next, PERMISSION_MODE_SETTLE_MS);
      }
      return;
    }
    if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); void send(); }
  }

  async function doFork(id?: string) {
    const target = id ?? convId;
    if (!target) return;
    try {
      const r = await forkConversation(target);
      notify('success', `Duplicated (${r.messages} messages).`);
      refreshConvs(r.forked);
      refreshSessions();
    } catch (e: any) {
      notify('error', e?.message ?? 'Duplicate failed.');
    }
  }

  async function doShare(targetId: string, o: ShareOptions) {
    if (!convId) return;
    try {
      const r = await shareConversation(convId, targetId, o.messages ? o.turns : 1, o.note, {
        messages: o.messages, summary: o.summary, attachments: o.attachments, memory: o.memory,
      });
      notify('success', `Shared ${r.turns} turns.`);
      setShowShare(false);
    } catch (e: any) {
      notify('error', e?.message ?? 'Share failed.');
    }
  }

  async function doCompact() {
    if (!convId || compacting) return;
    setCompacting(true);
    try {
      const r = await compactConversation(convId);
      notify(r.status === 'compacted' ? 'success' : 'info',
        r.status === 'compacted'
          ? `Compacted: ${r.before_chars} → ${r.after_chars} chars (saved ${r.saved_pct}%).`
          : r.text);
      await reloadMsgs(convId);
    } catch (e: any) {
      notify('error', e?.message ?? 'Compact failed.');
    } finally {
      setCompacting(false);
    }
  }

  async function doExport(id?: string) {
    const target = id ?? convId;
    if (!target) return;
    try {
      const data = await exportConversation(target);
      const blob = new Blob([JSON.stringify(data, null, 2)], { type: 'application/json' });
      const a = document.createElement('a');
      a.href = URL.createObjectURL(blob);
      a.download = `session-${target.slice(0, 8)}.json`;
      a.click();
      URL.revokeObjectURL(a.href);
      const requests = Array.isArray(data?.model_requests) ? data.model_requests.length : 0;
      notify('success', requests > 0
        ? `Session exported, including ${requests} model request record${requests === 1 ? '' : 's'}. These can contain file contents the assistant read; check before sharing.`
        : 'Session exported.');
    } catch (e: any) {
      notify('error', e?.message ?? 'Export failed.');
    }
  }

  function togglePin(id: string) {
    setPinnedIds((prev) => {
      const next = prev.includes(id) ? prev.filter((p) => p !== id) : [...prev, id];
      localStorage.setItem('companion.pinned', JSON.stringify(next));
      return next;
    });
  }

  async function doRename(id: string, title: string) {
    const t = title.trim().slice(0, 60);
    if (!t) { setRenaming(null); return; }
    try {
      await patchConversation(id, { title: t });
      setRenaming(null);
      refreshConvs();
    } catch (e: any) {
      notify('error', e?.message ?? 'Rename failed.');
    }
  }

  function doClear() {
    if (!convId) {
      void newChat();
      return;
    }
    void send('/clear');
  }

  async function maybeAutoTitle(cid: string, userText: string) {
    // The latest list, not the one this send started with: a session created
    // moments ago is only in the newer one.
    const t = autoTitle(convsRef.current.find((c) => c.id === cid)?.title, userText);
    if (!t) return;
    try {
      await patchConversation(cid, { title: t });
      refreshConvs();
    } catch { /* title is cosmetic; never fail the turn */ }
  }

  function doDelete(id: string) {
    const target = convs.find((conversation) => conversation.id === id);
    setConfirmState({
      title: 'Delete this conversation?',
      body: `“${target?.title || 'Untitled'}” and its history will be removed from this PC. This can’t be undone.`,
      action: 'Delete conversation',
      icon: 'trash',
      onConfirm: () => {
        void (async () => {
          try {
            await deleteConversation(id);
            if (conversationRef.current === id) { conversationRef.current = null; setConvId(null); setMsgs([]); setCtx(null); }
            refreshConvs();
            refreshSessions();
            notify('success', 'Conversation deleted.');
          } catch (e: any) {
            notify('error', e?.message ?? 'Delete failed.');
          }
        })();
      },
    });
  }

  function confirmDeleteModel(model: ModelMeta) {
    setConfirmState({
      title: `Delete ${model.name}?`,
      body: 'This permanently removes the model file from the models folder. Conversations that used it are kept.',
      action: 'Delete model',
      icon: 'trash',
      onConfirm: () => {
        deleteModel(model.id).then(() => { notify('success', `Deleted ${model.name}.`); refreshModels(); }).catch((e) => notify('error', e.message));
      },
    });
  }

  async function setPriority(p: string) {
    if (!convId) return;
    try {
      await patchSession(convId, { priority: p });
      refreshSessions();
    } catch (e: any) {
      notify('error', e?.message ?? 'Priority update failed.');
    }
  }

  const loadedModel = models.find((m) => m.loaded)?.id ?? '';
  // The selection always follows the model that is actually loaded (including
  // one loaded from another window), so the UI never offers a model you are
  // not using as if it were the current one.
  useEffect(() => {
    if (loadedModel) setModelId(loadedModel);
  }, [loadedModel]);
  const activeConv = convs.find((c) => c.id === convId) ?? null;
  const needsPrepare = !!(
    activeConv?.last_model &&
    loadedModel &&
    activeConv.last_model !== loadedModel
  );

  /** Load/start with the agent-execution guard. */
  async function guardedSwitch(kind: 'load' | 'start', id: string, force: boolean) {
    setLoadingModel(true);
    try {
      // The load reports what the person should know about it (a CPU
      // fallback, a context larger than the model supports).
      const started: { notices?: string[] } | undefined = kind === 'load'
        ? await loadModel(id, force)
        : await inferenceStart(id, force);
      await refreshModels();
      const status = await inferenceStatus();
      setInf(status);
      const name = models.find((model) => model.id === id)?.name ?? id;
      if (!force) notify('success', kind === 'load' ? `${name} is loaded and ready.` : 'Inference started.');
      const notices = started?.notices ?? [];
      for (const notice of notices) notify('warning', notice);
      if (status.runtime_notice && !notices.includes(status.runtime_notice)) notify('info', status.runtime_notice);
    } catch (e: any) {
      if (e?.status === 409 && !force) {
        setGuard({ kind, id, detail: e.message });
      } else {
        notify('error', e.message);
      }
    } finally {
      setLoadingModel(false);
    }
  }

  async function unloadSelectedModel() {
    setLoadingModel(true);
    try {
      await unloadModels();
      await refreshModels();
      setInf(await inferenceStatus());
      notify('info', 'Model ejected. GPU memory is free again.');
    } catch (e: any) {
      notify('error', e?.message ?? 'Could not unload the model.');
    } finally {
      setLoadingModel(false);
    }
  }

  async function reloadSelectedModel() {
    if (!modelId) return;
    setLoadingModel(true);
    try {
      await unloadModels();
    } catch { /* loading below reports the useful error */ }
    setLoadingModel(false);
    await guardedSwitch('load', modelId, false);
  }

  // Loading, switching and ejecting interrupt whatever the model is doing for
  // you. Starting a model when none is running is safe in one click; anything
  // that replaces or removes a running model asks first.
  function runningModel() {
    return inf?.running ? models.find((model) => model.loaded) : undefined;
  }

  function requestLoad(id: string) {
    const current = runningModel();
    const target = models.find((model) => model.id === id);
    if (current?.id === id) return;
    if (current) {
      setConfirmState({
        title: `Switch to ${target?.name ?? id}?`,
        body: `${current.name} will be unloaded first, so it stops answering while ${target?.name ?? 'the new model'} loads. Your conversations stay as they are; the next reply rebuilds its context with the new model.`,
        action: 'Switch model',
        icon: 'refresh',
        onConfirm: () => { setModelId(id); void guardedSwitch('load', id, false); },
      });
      return;
    }
    setModelId(id);
    void guardedSwitch('load', id, false);
  }

  function chooseModel(id: string) {
    if (runningModel()) requestLoad(id);
    else setModelId(id);
  }

  function requestEject() {
    const current = runningModel();
    setConfirmState({
      title: `Eject ${current?.name ?? 'the model'}?`,
      body: 'This frees its memory. The model has to load again before it can answer your next message.',
      action: 'Eject model',
      icon: 'eject',
      onConfirm: () => void unloadSelectedModel(),
    });
  }

  function requestReload() {
    const current = runningModel();
    setConfirmState({
      title: `Reload ${current?.name ?? 'the model'}?`,
      body: 'It is unavailable for a moment while it loads again. Use this if replies have become stuck or settings changed.',
      action: 'Reload model',
      icon: 'refresh',
      onConfirm: () => void reloadSelectedModel(),
    });
  }

  async function guardWait() {
    if (!guard) return;
    const g = guard;
    setGuard(null);
    notify('info', 'Waiting for the agent to reach a safe point…');
    for (let i = 0; i < 120; i++) {
      try {
        const runs = await agentRuns();
        const live = runs.filter((r) => !['COMPLETED', 'FAILED', 'CANCELLED'].includes(r.state));
        if (live.length === 0) break;
      } catch { /* keep waiting */ }
      await new Promise((r) => setTimeout(r, 2000));
    }
    void guardedSwitch(g.kind, g.id, false);
  }

  async function guardStopSwitch() {
    if (!guard) return;
    const g = guard;
    setGuard(null);
    try {
      const runs = await agentRuns();
      for (const r of runs) {
        if (!['COMPLETED', 'FAILED', 'CANCELLED'].includes(r.state)) {
          await stopAgent(r.id).catch(() => {});
        }
      }
      await guardedSwitch(g.kind, g.id, true);
    } catch (e: any) {
      notify('error', e.message);
    }
  }

  function stopAgentRuns() {
    void agentRuns().then(async (runs) => {
      await Promise.all(runs.filter((run) => run.conversation_id === convId && !['COMPLETED', 'FAILED', 'CANCELLED'].includes(run.state)).map((run) => stopAgent(run.id)));
      if (conversationRef.current === convId) refreshAgentActivity.current();
      if (convId) void reloadMsgs(convId);
    }).catch((error) => notify('error', error.message));
  }

  function stopGeneration() {
    abort.current?.abort();
    void stopChat()
      .then((o) => {
        if (o.stopped) notify('info', `Stopped — partial reply kept (${o.chars_kept} characters).`);
      })
      .catch(() => { /* fetch abort still applies */ });
  }

  const modeConvs = convs.filter((c) => (c.mode || 'chat') === mode);
  const pinnedConvs = modeConvs.filter((c) => pinnedIds.includes(c.id));
  const filterText = sessionFilter.trim().toLowerCase();
  const listConvs = modeConvs.filter((c) => !pinnedIds.includes(c.id) && (!filterText || c.title.toLowerCase().includes(filterText)));
  const visibleWork = visibleWorkActivity(convId, busy, chatActivity, agentBusy, agentActivity);
  const selectedModel = models.find((model) => model.id === modelId);
  const loadedMeta = models.find((model) => model.loaded && inf?.running);
  const modelReady = !!inf?.running && loadedModel === modelId && !!modelId;
  const headWorkspace = workspaces.find((workspace) => workspace.id === (activeConv?.workspace ?? wsId));
  const branch = useWorkspaceBranch(mode === 'code' ? headWorkspace?.id : undefined);
  const rail = collapsed && !narrow;
  const anySessionLive = sessions.some((s) => s.id !== convId && (s.activity === 'thinking' || s.activity === 'tool'));
  const anySessionWaiting = sessions.some((s) => s.id !== convId && s.activity === 'waiting');
  const machineActivity: MachineActivity = busy ? 'generating'
    : agentBusy ? (agentPhase === 'WAITING_PERMISSION' || !inf?.running ? 'waiting' : 'agent')
      : anySessionWaiting ? 'waiting'
        : anySessionLive ? 'agent'
          : 'idle';
  const machine = machineState(backendUp, loadingModel, machineActivity, !!loadedMeta);
  const lastReplyTps = useMemo(() => {
    const last = [...msgs].reverse().find((message) => message.role === 'assistant' && perfMap[message.id]?.tps != null);
    return last ? perfMap[last.id].tps : null;
  }, [msgs, perfMap]);
  const lastAssistantId = [...msgs].reverse().find((message) => message.role === 'assistant')?.id;
  const currentPriority = sessions.find((sn) => sn.id === convId)?.priority ?? 'normal';
  const modelName = (id?: string) => id ? models.find((model) => model.id === id)?.name ?? id : undefined;

  const paletteActions: QuickAction[] = [
    { id: 'new', group: 'Actions', icon: 'plus', label: mode === 'code' ? 'New task' : 'New chat', detail: mode === 'code' ? 'Start a code session in the current project' : 'Start a fresh conversation', run: () => void newChat() },
    { id: 'project', group: 'Actions', icon: 'folderPlus', label: 'Open a project', detail: 'Choose the files your coding agent can access', run: () => setProjectLauncherOpen(true) },
    { id: 'panel', group: 'Actions', icon: 'panelRight', label: rightOpen ? 'Hide inspector' : 'Show inspector', detail: 'Activity, files and context beside the conversation', run: () => { setTab('chat'); setRightOpen((open) => !open); } },
    ...(selectedModel && !modelReady ? [{ id: 'load-model', group: 'Actions', icon: 'power' as IconName, label: `Load ${selectedModel.name}`, detail: 'Move the selected model into memory', run: () => requestLoad(selectedModel.id) }] : []),
    ...(loadedMeta ? [{ id: 'eject-model', group: 'Actions', icon: 'eject' as IconName, label: `Eject ${loadedMeta.name}`, detail: 'Unload the model and free GPU memory', run: () => requestEject() }] : []),
    ...(['dark', 'light', 'system'] as Theme[]).filter((value) => value !== theme).map((value) => ({ id: `theme-${value}`, group: 'Actions', icon: (value === 'dark' ? 'moon' : value === 'light' ? 'sun' : 'monitor') as IconName, label: value === 'system' ? 'Match system theme' : `Use ${value} theme`, detail: `Current theme: ${theme}`, run: () => setTheme(value) })),
    { id: 'mode-chat', group: 'Go to', icon: 'chat', label: 'Chat', detail: 'Conversations', run: () => switchMode('chat') },
    { id: 'mode-code', group: 'Go to', icon: 'code', label: 'Code', detail: 'Project sessions', run: () => switchMode('code') },
    ...WORKBENCH_DESTINATIONS.map(({ id, label }) => ({ id, group: 'Go to', icon: PAGE_META[id].icon, label, detail: PAGE_META[id].description, run: () => { setTab(id); setMobileNav(false); } })),
    ...convs.map((conversation) => ({ id: conversation.id, group: 'Sessions', icon: (conversation.mode === 'code' ? 'code' : 'chat') as IconName, label: conversation.title || 'Untitled', detail: conversation.mode === 'code' ? `Code session${workspaces.find((w) => w.id === conversation.workspace) ? ` · ${workspaces.find((w) => w.id === conversation.workspace)!.name}` : ''}` : 'Conversation', run: () => void selectConv(conversation.id) })),
  ];

  const renderRow = (c: Conversation) => {
    const sess = sessions.find((sn) => sn.id === c.id);
    const isActive = convId === c.id;
    const waiting = sess?.activity === 'waiting' || (isActive && agentBusy && agentPhase === 'WAITING_PERMISSION');
    const live = !waiting && ((sess?.activity === 'thinking' || sess?.activity === 'tool') || (isActive && (busy || agentBusy)));
    const stale = !recoveryOff && !!recovery?.stale.some((item) => item.id === c.id);
    if (renaming?.id === c.id && renaming.where === 'list') {
      return (
        <div key={c.id} className="sb-row">
          <input
            className="sb-rename"
            autoFocus
            value={renaming.draft}
            onChange={(e) => setRenaming({ id: c.id, draft: e.target.value, where: 'list' })}
            onKeyDown={(e) => {
              if (e.key === 'Enter') void doRename(c.id, renaming.draft);
              if (e.key === 'Escape') setRenaming(null);
            }}
            onBlur={() => setRenaming(null)}
            aria-label="Rename session"
          />
        </div>
      );
    }
    const status = live ? 'Working' : waiting ? 'Waiting for approval' : sess?.activity === 'error' ? 'Error' : stale ? 'Used another model' : '';
    return (
      <div key={c.id} className={`sb-row${isActive ? ' active' : ''}`}>
        <button type="button" className="sb-row-main" onClick={() => void selectConv(c.id)} title={status ? `${c.title} · ${status}` : c.title} aria-current={isActive ? 'true' : undefined}>
          <span className="sb-row-status" aria-hidden="true">
            {live ? <Lamp state="live" pulse /> : waiting || stale ? <Lamp state="caution" /> : sess?.activity === 'error' ? <Lamp state="error" /> : null}
          </span>
          <span className="sb-row-title">{c.title || 'Untitled'}</span>
          {status && <span className="sr-only">, {status}</span>}
        </button>
        <SessionMenu
          pinned={pinnedIds.includes(c.id)}
          onRename={() => setRenaming({ id: c.id, draft: c.title || '', where: 'list' })}
          onDuplicate={() => { void selectConv(c.id); setTimeout(() => void doFork(c.id), 50); }}
          onTogglePin={() => togglePin(c.id)}
          onExport={() => void doExport(c.id)}
          onClose={() => doDelete(c.id)}
        />
      </div>
    );
  };

  const workLabel = visibleWork?.kind === 'agent'
    ? agentPhase === 'ROUTING' ? 'Understanding your request…' : !inf?.running ? 'Model stopped · this run needs attention' : agentPhase === 'WAITING_PERMISSION' ? 'Waiting for your approval' : agentPhase === 'COMPACTING' ? 'Compacting context… the run resumes on its own' : agentPhase === 'EXECUTING_TOOL' ? 'Working through the project…' : agentPhase === 'OBSERVING' ? 'Reviewing the results…' : 'Planning the next step…'
    : statusLine || (generationPhase === 'compacting' ? 'Compacting the conversation before replying…' : generationPhase === 'thinking' ? `${loadedMeta?.name ?? 'The model'} is thinking…` : generationPhase === 'responding' ? `${loadedMeta?.name ?? 'The model'} is writing…` : 'Processing your request…');

  const receipt: { text: string; warn?: boolean; icon?: IconName }[] = [];
  if (search) receipt.push({ text: 'Web search is on for your next message — queries leave this PC', warn: true, icon: 'globe' });
  if (usage && !visibleWork) {
    receipt.push({ text: `Last reply · ${usage.prompt_tokens.toLocaleString()} in / ${usage.generated_tokens.toLocaleString()} out` });
    if (usage.stopped) receipt.push({ text: 'Stopped early · partial reply kept', warn: true });
    if (usage.truncated && !usage.stopped) receipt.push({ text: 'Reached the output limit · ask it to continue', warn: true });
    if (usage.reasoning && usage.reasoning !== 'off') receipt.push({ text: `Reasoned (${usage.reasoning})`, icon: 'sparkle' });
    if (usage.sources) receipt.push({ text: `${usage.sources} source${usage.sources === 1 ? '' : 's'}`, icon: 'globe' });
    if (usage.vision === 'unsupported') receipt.push({ text: 'Images skipped — this model has no vision', warn: true, icon: 'image' });
    if (usage.tool_rounds) receipt.push({ text: `Read ${usage.tool_rounds} file${usage.tool_rounds === 1 ? '' : 's'}`, icon: 'eye' });
  }

  return (
    <div className={`shell${rail ? ' rail' : ''}${rightOpen && tab === 'chat' ? '' : ' no-right'}${mobileNav ? ' mobile-nav' : ''}`}>
      {paletteOpen && <CommandPalette onClose={() => setPaletteOpen(false)} actions={paletteActions} />}
      {mobileNav && <button type="button" className="nav-scrim" aria-label="Close navigation" onClick={() => setMobileNav(false)} />}
      <a href="#composer" className="skip-link" onClick={(e) => { e.preventDefault(); composerRef.current?.focus(); }}>
        Skip to composer
      </a>

      <aside className="sidebar" aria-label="Navigation">
        <div className="sb-top">
          <div className="sb-brand">
            {rail ? (
              <span className="tally-mark" aria-hidden="true"><Lamp state={machine.state} pulse={machine.state === 'live'} /></span>
            ) : (
              <div className="sb-wordmark">
                <span className="tally-mark" aria-hidden="true"><Lamp state={machine.state} pulse={machine.state === 'live'} /></span>
                <strong>Companion</strong>
              </div>
            )}
            <IconButton
              icon="panelLeft"
              label={narrow ? 'Close navigation' : collapsed ? 'Expand sidebar' : 'Collapse sidebar'}
              tipSide={rail ? 'right' : 'bottom-end'}
              onClick={() => { if (narrow) setMobileNav(false); else setCollapsed((v) => !v); }}
            />
          </div>
          {rail ? (
            <>
              <IconButton icon="plus" label={mode === 'code' ? 'New task' : 'New chat'} tipSide="right" onClick={() => void newChat()} />
              <IconButton icon="search" label="Search" tipSide="right" onClick={() => setPaletteOpen(true)} />
              <div className="sb-modes-rail" role="group" aria-label="Work mode">
                <IconButton icon="chat" label="Chat" pressed={mode === 'chat' && tab === 'chat'} tipSide="right" onClick={() => switchMode('chat')} />
                <IconButton icon="code" label="Code" pressed={mode === 'code' && tab === 'chat'} tipSide="right" onClick={() => switchMode('code')} />
              </div>
            </>
          ) : (
            <>
              <Button className="sb-new" icon="plus" onClick={() => void newChat()}>New {mode === 'code' ? 'task' : 'chat'}</Button>
              <button type="button" className="sb-search" onClick={() => setPaletteOpen(true)}>
                <Icon name="search" size={15} />
                <span>Search</span>
                <Kbd>{shortcutLabel(paletteShortcut)}</Kbd>
              </button>
              <div className="sb-modes" role="tablist" aria-label="Work mode">
                <button type="button" role="tab" aria-selected={mode === 'chat'} onClick={() => switchMode('chat')} title="Chat (Ctrl 1)"><Icon name="chat" size={15} />Chat</button>
                <button type="button" role="tab" aria-selected={mode === 'code'} onClick={() => switchMode('code')} title="Code (Ctrl 2)"><Icon name="code" size={15} />Code</button>
              </div>
              {mode === 'code' && (
                <div className="project-switch">
                  <button
                    type="button"
                    className={`project-switch-btn${workspaces.some((workspace) => workspace.id === wsId) ? '' : ' empty'}`}
                    aria-haspopup="menu"
                    aria-expanded={projectMenuOpen}
                    aria-label="Active project"
                    disabled={busy || agentBusy}
                    onClick={() => setProjectMenuOpen((open) => !open)}
                  >
                    <Icon name="folder" size={16} />
                    <span className="project-switch-copy">
                      <strong>{workspaces.find((workspace) => workspace.id === wsId)?.name ?? 'Choose a project'}</strong>
                      {workspaces.find((workspace) => workspace.id === wsId) && <small>{workspaces.find((workspace) => workspace.id === wsId)!.path}</small>}
                    </span>
                    <Icon name="chevronsUpDown" size={14} />
                  </button>
                  <Popover open={projectMenuOpen} onClose={() => setProjectMenuOpen(false)} label="Projects" side="bottom" align="start">
                    {workspaces.length > 0 && <PopLabel>Projects</PopLabel>}
                    {workspaces.map((workspace) => (
                      <PopItem key={workspace.id} checked={workspace.id === wsId} onClick={() => { setProjectMenuOpen(false); void changeWorkspace(workspace.id); }}>
                        {workspace.name}
                      </PopItem>
                    ))}
                    {workspaces.length > 0 && <PopDivider />}
                    <PopItem icon="folderPlus" onClick={() => { setProjectMenuOpen(false); setProjectLauncherOpen(true); }}>Open or create a project…</PopItem>
                  </Popover>
                </div>
              )}
              {recovery && !recoveryOff && (
                <RecoveryBanner
                  recovery={recovery}
                  onResume={(id) => {
                    refreshConvs(id);
                    setTab('chat');
                    notify('info', 'Session restored. The loaded model will rebuild context when you send a message.');
                  }}
                  onDiscard={(id) => discardStale(id, loadedModel).then(() => {
                    notify('info', 'Other-model marker cleared.');
                    getRecovery().then(setRecovery).catch(() => {});
                    refreshConvs();
                  }).catch((e) => notify('error', e.message))}
                  onDismiss={() => setRecoveryOff(true)}
                />
              )}
            </>
          )}
        </div>

        {rail ? <div className="sb-rail-spacer" /> : (
          <nav className="sb-list" aria-label={mode === 'code' ? 'Code sessions' : 'Conversations'}>
            {pinnedConvs.length > 0 && (
              <>
                <div className="sb-group-label"><span className="eyebrow">Pinned</span></div>
                {pinnedConvs.map(renderRow)}
              </>
            )}
            <div className="sb-group-label"><span className="eyebrow">{mode === 'code' ? 'Tasks' : 'Chats'}</span>{modeConvs.length > 0 && <span className="readout muted" style={{ fontSize: 11.5 }}>{modeConvs.length}</span>}</div>
            {(modeConvs.length > 8 || sessionFilter) && (
              <div className="sb-filter">
                <Icon name="search" size={13} />
                <input aria-label="Filter sessions" placeholder="Filter" value={sessionFilter} onChange={(event) => setSessionFilter(event.target.value)} />
              </div>
            )}
            {listConvs.map(renderRow)}
            {modeConvs.length === 0 && <p className="sb-empty">{mode === 'code' ? 'No tasks yet. Start one to work in a project.' : 'No conversations yet. Start one above.'}</p>}
            {modeConvs.length > 0 && listConvs.length === 0 && filterText && <p className="sb-empty">Nothing matches “{sessionFilter}”.</p>}
          </nav>
        )}

        <nav className="sb-utility" aria-label="Manage Companion">
          {WORKBENCH_DESTINATIONS.map(({ id, label }) => (
            <button
              type="button"
              key={id}
              className="sb-link"
              aria-label={label}
              aria-current={tab === id ? 'page' : undefined}
              data-tip={rail ? label : undefined}
              data-tip-side={rail ? 'right' : undefined}
              onClick={() => { setTab(id); setMobileNav(false); }}
            >
              <Icon name={PAGE_META[id].icon} size={16} />
              {!rail && <span>{label}</span>}
            </button>
          ))}
        </nav>

        <Rig
          models={models}
          modelId={modelId}
          inf={inf}
          backendUp={backendUp}
          loadingModel={loadingModel}
          activity={machineActivity}
          liveTps={liveTps}
          lastTps={lastReplyTps}
          phaseLabel={busy ? (generationPhase === 'compacting' ? 'Compacting context…' : generationPhase === 'thinking' ? 'Thinking…' : generationPhase === 'responding' ? 'Writing…' : 'Reading your message…') : agentBusy ? 'Agent working…' : undefined}
          collapsed={rail}
          onSelect={chooseModel}
          onLoad={requestLoad}
          onUnload={requestEject}
          onReload={requestReload}
          onOpenModels={() => { setTab('models'); setMobileNav(false); }}
          onOpenResources={() => { setTab('resources'); setMobileNav(false); }}
          notify={(kind, text) => notify(kind, text)}
        />
      </aside>

      {projectLauncherOpen && (
        <ProjectLauncher
          recent={workspaces}
          onChoose={(workspace) => {
            setProjectLauncherOpen(false);
            setWorkspaces((items) => items.some((item) => item.id === workspace.id) ? items : [...items, workspace]);
            void changeWorkspace(workspace.id);
          }}
          onClose={() => setProjectLauncherOpen(false)}
          notify={notify}
        />
      )}

      <div className="center" ref={centerRef}>
        <Toasts toasts={toasts} dismiss={dismissToast} />
        <header className="head">
          <div className="head-mobile">
            <IconButton icon="menu" label="Open navigation" tip={false} aria-expanded={mobileNav} onClick={() => setMobileNav((v) => !v)} />
            <Lamp state={machine.state} pulse={machine.state === 'live'} />
          </div>
          <div className="head-title">
            {tab !== 'chat' ? (
              <h1>{PAGE_META[tab].title}</h1>
            ) : renaming?.where === 'head' && renaming.id === convId ? (
              <input
                className="head-rename"
                autoFocus
                value={renaming.draft}
                aria-label="Rename session"
                onChange={(e) => setRenaming({ id: renaming.id, draft: e.target.value, where: 'head' })}
                onKeyDown={(e) => {
                  if (e.key === 'Enter') void doRename(renaming.id, renaming.draft);
                  if (e.key === 'Escape') setRenaming(null);
                }}
                onBlur={() => void doRename(renaming.id, renaming.draft)}
              />
            ) : (
              <h1>
                {activeConv
                  ? <button type="button" onClick={() => setRenaming({ id: activeConv.id, draft: activeConv.title || '', where: 'head' })} title="Rename">{activeConv.title || 'Untitled'}</button>
                  : mode === 'code' ? 'New task' : 'New chat'}
              </h1>
            )}
            <div className="head-sub">
              {tab !== 'chat' ? (
                <span>{PAGE_META[tab].description}</span>
              ) : mode === 'code' ? (
                headWorkspace ? (
                  <>
                    <Icon name="folder" size={13} />
                    <span>{headWorkspace.name}</span>
                    <span className="head-path"><span className="sep" aria-hidden="true" /><span className="path" title={headWorkspace.path}>{headWorkspace.path}</span></span>
                    {branch && <><span className="sep" aria-hidden="true" /><Icon name="branch" size={13} /><span>{branch}</span></>}
                  </>
                ) : <span>No project selected</span>
              ) : (
                <>
                  <Icon name="lock" size={12} />
                  <span>{msgs.length ? `${msgs.length} message${msgs.length === 1 ? '' : 's'} · stays on this PC` : 'Stays on this PC'}</span>
                </>
              )}
            </div>
          </div>
          {tab === 'chat' && (
            <div className="head-actions">
              {mode === 'code' && activeConv?.workspace && (
                <>
                  <ProjectActions
                    busy={busy || agentBusy}
                    onChanges={() => setDiffWs(activeConv.workspace!)}
                    onCommand={(command) => { if (command === '/run ') { setInput(command); composerRef.current?.focus(); } else void send(command); }}
                  />
                  <span className="head-divider" aria-hidden="true" />
                </>
              )}
              <span className="pop-anchor">
                <IconButton icon="more" label="Session actions" tipSide="bottom-end" aria-haspopup="menu" aria-expanded={sessionMenuOpen} disabled={!convId} onClick={() => setSessionMenuOpen((v) => !v)} />
                <Popover open={sessionMenuOpen} onClose={() => setSessionMenuOpen(false)} label="Session actions" side="bottom" align="end">
                  <PopItem icon="pencil" onClick={() => { setSessionMenuOpen(false); if (convId) setRenaming({ id: convId, draft: activeConv?.title ?? '', where: 'head' }); }}>Rename</PopItem>
                  <PopItem icon="fork" onClick={() => { setSessionMenuOpen(false); void doFork(); }}>Duplicate</PopItem>
                  <PopItem icon="share" onClick={() => { setSessionMenuOpen(false); setShowShare(true); }}>Share context…</PopItem>
                  <PopItem icon="download" onClick={() => { setSessionMenuOpen(false); void doExport(); }}>Export</PopItem>
                  <PopDivider />
                  <PopItem icon="layers" disabled={compacting} onClick={() => { setSessionMenuOpen(false); void doCompact(); }}>Compact context</PopItem>
                  <PopItem icon="refresh" onClick={() => { setSessionMenuOpen(false); doClear(); }}>Clear session</PopItem>
                  <PopItem icon="shield" onClick={() => { setSessionMenuOpen(false); setPermOpen(true); }}>Permissions…</PopItem>
                  <PopDivider />
                  <PopLabel>Scheduling priority</PopLabel>
                  {PRIORITIES.map((priority) => (
                    <PopItem key={priority.id} checked={currentPriority === priority.id} onClick={() => { setSessionMenuOpen(false); void setPriority(priority.id); }}>{priority.label}</PopItem>
                  ))}
                  <PopDivider />
                  <PopItem icon="trash" danger onClick={() => { setSessionMenuOpen(false); if (convId) doDelete(convId); }}>Delete…</PopItem>
                </Popover>
              </span>
              <IconButton icon="panelRight" label={rightOpen ? 'Hide inspector' : 'Show inspector'} pressed={rightOpen} tipSide="bottom-end" onClick={() => setRightOpen((v) => !v)} />
            </div>
          )}
        </header>

        {backendUp === false && (
          <Notice tone="error" className="global-notice" title="The local runtime isn’t responding">
            Close this window and start Companion again with its start script: <StartScripts />. Your conversations are safe on disk.
          </Notice>
        )}

        {permOpen && (
          <PermissionsModal
            onClose={() => setPermOpen(false)}
            onOpenSettings={() => setTab('settings')}
          />
        )}

        {tab === 'chat' && (
          <>
            {showShare && convId && (
              <ShareDialog
                targets={convs.filter((c) => c.id !== convId).map((c) => ({ id: c.id, label: `${c.mode === 'code' ? 'Code' : 'Chat'} · ${c.title}` }))}
                onShare={(t, o) => void doShare(t, o)}
                onClose={() => setShowShare(false)}
              />
            )}
            <div className="stage">
              {historyLoading ? (
                <div className="history-loading" role="status" aria-label="Opening conversation"><i /><i /><i /><i /></div>
              ) : msgs.length === 0 ? (
                <Welcome
                  mode={mode}
                  machine={machine.state}
                  loaded={loadedMeta}
                  selected={selectedModel}
                  contextSize={loadedMeta ? inf?.context_size ?? null : selectedModel?.context_length ?? null}
                  workspace={workspaces.find((workspace) => workspace.id === (activeConv?.workspace ?? wsId))}
                  workspaces={workspaces}
                  branch={branch}
                  onStarter={(text) => { setInput(text); requestAnimationFrame(() => composerRef.current?.focus()); }}
                  onLoad={() => { if (selectedModel) requestLoad(selectedModel.id); }}
                  onChooseModel={() => setTab('models')}
                  onChooseProject={() => setProjectLauncherOpen(true)}
                  onPickProject={(workspace) => void changeWorkspace(workspace.id)}
                />
              ) : (
                <div className="transcript" ref={transcriptRef} onScroll={(event) => { const view = event.currentTarget; followOutput.current = view.scrollHeight - view.scrollTop - view.clientHeight < 100; setShowLatest(!followOutput.current); }}>
                  <div className="transcript-inner">
                    {msgs.length > msgLimit && (
                      <Button variant="ghost" size="sm" icon="history" className="older-toggle" onClick={() => setMsgLimit((l) => l + 200)}>
                        Show {msgs.length - msgLimit} older message{msgs.length - msgLimit === 1 ? '' : 's'}
                      </Button>
                    )}
                    {msgs.slice(-msgLimit).filter((message) => !(agentBusy && message.id === focusRun)).map((m) => (
                      editing && editing.mid === m.id ? (
                        <div key={m.id} className="message-row user">
                          <div className="msg-edit">
                            <textarea
                              value={editing.draft}
                              autoFocus
                              onChange={(e) => setEditing({ mid: m.id, draft: e.target.value })}
                              onKeyDown={(e) => { if (e.key === 'Escape') setEditing(null); if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) void saveEdit(); }}
                              rows={4}
                              aria-label="Edit message"
                            />
                            <div className="msg-edit-actions">
                              <span className="help">Saving removes the replies that came after this message.</span>
                              <Button variant="ghost" size="sm" onClick={() => setEditing(null)}>Cancel</Button>
                              <Button size="sm" onClick={() => void saveEdit()} disabled={!editing.draft.trim()}>Save</Button>
                            </div>
                          </div>
                        </div>
                      ) : (
                        <div key={m.id} className={`message-row ${m.role}`}>
                          <MessageView
                            sessionMode={mode}
                            activities={m.activities}
                            role={m.role}
                            text={m.text}
                            time={m.time}
                            streaming={busy && m.id === msgs[msgs.length - 1]?.id && m.role === 'assistant'}
                            tps={
                              busy && m.id === msgs[msgs.length - 1]?.id && m.role === 'assistant'
                                ? liveTps
                                : m.role === 'assistant'
                                  ? (perfMap[m.id]?.tps ?? null)
                                  : null
                            }
                            live={busy && m.id === msgs[msgs.length - 1]?.id && m.role === 'assistant'}
                            timing={m.role === 'assistant' ? perfMap[m.id]?.timing : undefined}
                            legacyRate={m.role === 'assistant' && perfMap[m.id]?.legacy}
                            showMetrics={showGenerationSpeed}
                            detailedMetrics={showDetailedMetrics}
                            byline={m.role === 'assistant' ? modelName(perfMap[m.id]?.model) : undefined}
                            thinking={m.role === 'assistant' ? thinkingMap[m.id] : undefined}
                            agentRunId={m.id}
                            onOpenAgentActivity={(runId) => {
                              setFocusRun(runId);
                              setRightTab('activity');
                              setRightOpen(true);
                            }}
                            onEdit={!busy && !agentBusy && m.role === 'user' && !m.id.startsWith('tmp-') ? () => setEditing({ mid: m.id, draft: m.text }) : undefined}
                            onRegenerate={!busy && !agentBusy && m.role === 'assistant' && m.id === lastAssistantId ? regenerate : undefined}
                          />
                        </div>
                      )
                    ))}
                    {mode === 'code' && (
                      <AgentChatProgress
                        convId={convId}
                        runtimeRunning={!!inf?.running}
                        onContextUsage={(event, runId) => { if (conversationRef.current === convId) setCtx((context) => applyAgentContext(context, event, runId)); }}
                        focusRun={focusRun}
                        onOpenActivity={(runId) => {
                          setFocusRun(runId);
                          setRightTab('activity');
                          setRightOpen(true);
                        }}
                        onFinished={() => {
                          if (conversationRef.current !== convId) return;
                          refreshAgentActivity.current();
                          if (convId) void reloadMsgs(convId);
                          void refreshConvs();
                        }}
                        onApprovePlan={approvePlan}
                        onKeepPlanning={keepPlanning}
                      />
                    )}
                  </div>
                </div>
              )}
              {showLatest && <Button className="jump-latest" size="sm" icon="arrowDown" onClick={() => { followOutput.current = true; transcriptRef.current?.scrollTo({top:transcriptRef.current.scrollHeight,behavior:'smooth'}); setShowLatest(false); }}>Latest</Button>}
            </div>

            <div className="dock" ref={dockRef}>
              <div className="dock-inner">
                {needsPrepare && activeConv && !prepareDismissed[activeConv.id] && (
                  <PrepareBanner
                    convId={activeConv.id}
                    convTitle={activeConv.title}
                    lastModel={modelName(activeConv.last_model) ?? ''}
                    loadedModel={loadedModel}
                    loadedName={modelName(loadedModel)}
                    lastModelAvailable={models.some((model) => model.id === activeConv.last_model)}
                    onPrepared={() => refreshConvs(activeConv.id)}
                    onDismiss={() => setPrepareDismissed((items) => ({ ...items, [activeConv.id]: true }))}
                    onSwitchBack={() => {
                      const previous = activeConv.last_model ?? '';
                      if (!models.some((model) => model.id === previous)) {
                        notify('warning', `The previous model '${previous}' is no longer installed. Prepare this session for ${loadedModel} instead.`);
                        return;
                      }
                      requestLoad(previous);
                    }}
                    notify={(k, t) => (k === 'error' ? notify('error', t) : notify(k === 'success' ? 'success' : 'info', t))}
                  />
                )}
                <WorkStatus active={!!visibleWork} startedAt={visibleWork?.startedAt} waiting={visibleWork?.kind === 'agent' && (agentPhase === 'WAITING_PERMISSION' || !inf?.running)} label={workLabel} />
                {!visibleWork && receipt.length > 0 && (
                  <div className="dock-receipt">
                    {receipt.map((item) => <span key={item.text} className={item.warn ? 'warn' : ''}>{item.icon && <Icon name={item.icon} size={13} />}{item.text}</span>)}
                  </div>
                )}
                <AttachChips convId={convId} tick={attachTick} notify={(k, t) => notify(k === 'error' ? 'error' : 'info', t)} />
                <div
                  className={`composer${dragOver ? ' dragover' : ''}${machineActivity === 'waiting' && agentBusy ? ' waiting' : busy || agentBusy ? ' live' : ''}`}
                  onDragOver={(e) => { e.preventDefault(); setDragOver(true); }}
                  onDragLeave={() => setDragOver(false)}
                  onDrop={(e) => {
                    e.preventDefault();
                    setDragOver(false);
                    const f = e.dataTransfer.files?.[0];
                    if (f) {
                      if (!convId) notify('warning', 'Start a chat first, then drop files.');
                      else void attach(f);
                    }
                  }}
                >
                  <textarea
                    id="composer"
                    ref={composerRef}
                    value={input}
                    rows={1}
                    onChange={(e) => {
                      void onInput(e.target.value);
                      const el = e.target;
                      el.style.height = 'auto';
                      el.style.height = `${Math.min(el.scrollHeight, 240)}px`;
                    }}
                    onKeyDown={composerKey}
                    placeholder={dragOver ? 'Drop to attach' : mode === 'code' ? 'Ask, plan, or describe a change… (Shift+Tab: mode)' : 'Ask anything, or drop a file…'}
                    aria-label="Message composer"
                    disabled={loadingModel || agentBusy}
                  />
                  <div className="composer-bar">
                    <div className="composer-tools">
                      <input
                        ref={fileRef}
                        type="file"
                        hidden
                        onChange={(e) => { void attach(e.target.files?.[0]); e.target.value = ''; }}
                      />
                      <IconButton icon="paperclip" label={convId ? 'Attach a file' : 'Send a first message to attach files'} tipSide="top" disabled={!convId || busy || agentBusy} onClick={() => fileRef.current?.click()} />
                      {mode === 'code' && (
                        <span className={`permission-mode ${permissionMode}`} role="group" aria-label="Permission mode (Shift+Tab to cycle)" title={`${PROJECT_BOUNDARY_DESCRIPTION} ${SEARCH_PERMISSION_DESCRIPTION}`}>
                          {PERMISSION_MODES.map((option) => (
                            <button key={option} type="button" disabled={permissionModeBusy || busy || agentBusy} className={permissionMode === option ? 'active' : ''} aria-pressed={permissionMode === option} title={PERMISSION_MODE_DESCRIPTIONS[option]} onClick={() => void changePermissionMode(option)}>{PERMISSION_MODE_LABELS[option]}</button>
                          ))}
                        </span>
                      )}
                      <Toggle
                        on={reasoning}
                        icon="sparkle"
                        disabled={busy || agentBusy}
                        title={reasoningCapable ? 'Reasoning: extended response budget. Native thinking depends on the model and runtime.' : 'Extended response budget; native reasoning capability is unverified for this model.'}
                        onClick={() => setReasoning((v) => !v)}
                      >
                        Reasoning
                      </Toggle>
                      <Toggle on={search} icon="globe" tone="caution" disabled={busy || agentBusy} title="Web search: explicit internet access for this message" onClick={() => setSearch((v) => !v)}>
                        Web
                      </Toggle>
                    </div>
                    <div className="composer-end">
                      {/* Status only. Loading or switching models never happens from the composer. */}
                      {!loadedMeta && !loadingModel && backendUp !== false && !busy && !agentBusy && (
                        <span className="composer-hint" title="Load a model from the panel at the bottom left">
                          <Lamp state="off" />
                          No model loaded
                        </span>
                      )}
                      {loadingModel && <span className="composer-hint"><Lamp state="caution" pulse />Loading model…</span>}
                      <ContextGauge ctx={ctx} onCompact={() => void doCompact()} compacting={compacting} />
                      {agentBusy ? (
                        <button type="button" className={`send-btn stop${machineActivity === 'waiting' ? ' paused' : ''}`} aria-label="Stop agent" data-tip="Stop agent" data-tip-side="top" onClick={stopAgentRuns}><Icon name="stop" size={16} /></button>
                      ) : busy ? (
                        <button type="button" className="send-btn stop" aria-label="Stop generation" data-tip="Stop" data-tip-side="top" onClick={stopGeneration}><Icon name="stop" size={16} /></button>
                      ) : (
                        <button
                          type="button"
                          className={`send-btn${modelReady || input.trim().startsWith('/') ? '' : ' idle'}`}
                          onClick={() => void send()}
                          disabled={loadingModel || !input.trim()}
                          aria-label="Send message"
                          data-tip={loadingModel ? 'Model is loading' : modelReady ? 'Send (Enter)' : 'Load a model to send'}
                          data-tip-side="top"
                        >
                          <Icon name="arrowUp" size={17} strokeWidth={2.2} />
                        </button>
                      )}
                    </div>
                  </div>
                  {cmdMenu.length > 0 && (
                    <div className="cmdmenu" role="listbox" aria-label="Commands">
                      {cmdMenu.map((c, i) => (
                        <div
                          key={c.name}
                          role="option"
                          aria-selected={i === cmdSel}
                          className={`item${i === cmdSel ? ' sel' : ''}`}
                          onMouseEnter={() => setCmdSel(i)}
                          onMouseDown={(e) => { e.preventDefault(); setInput(`/${c.name} `); setCmdMenu([]); }}
                        >
                          <span className="name">/{c.name}</span>
                          <span className="desc">{c.description}</span>
                        </div>
                      ))}
                    </div>
                  )}
                </div>
              </div>
            </div>
          </>
        )}

        {tab === 'models' && (
          <ModelsPage
            models={models}
            loadingModel={loadingModel}
            downloads={downloads}
            notify={notify}
            onLoad={requestLoad}
            onDelete={confirmDeleteModel}
            refreshModels={refreshModels}
            refreshDownloads={refreshDownloads}
          />
        )}

        {tab === 'resources' && <ResourcesPanel notify={notify} />}

        {tab === 'system' && (
          <RuntimePage
            inf={inf}
            sys={sys}
            modelId={modelId}
            modelName={selectedModel?.name}
            backendUp={backendUp}
            notify={notify}
            onStart={() => void guardedSwitch('start', modelId, false)}
            setInf={setInf}
            setSys={setSys}
          />
        )}

        {tab === 'tools' && <ToolsPage registry={registry} wsId={wsId} notify={notify} onRefresh={() => void refreshRegistry()} />}

        {tab === 'settings' && <SettingsPanel setToasts={setToasts} />}

        {diffWs && <DiffModal wsId={diffWs} onClose={() => setDiffWs(null)} />}
      </div>

      {rightOpen && tab === 'chat' && (
        <RightPanel mode={mode} tab={rightTab} onTab={setRightTab} onClose={() => setRightOpen(false)}>
          <RightPanelTabs
            tab={rightTab}
            convId={convId}
            wsId={activeConv?.workspace ?? (mode === 'code' ? wsId : '')}
            wsPath={workspaces.find((w) => w.id === (activeConv?.workspace ?? wsId))?.path ?? ''}
            mode={mode}
            modelId={modelId}
            ctx={ctx}
            compacting={compacting}
            onCompact={() => void doCompact()}
            workspaces={workspaces}
            registry={registry}
            notify={notify}
            focusRun={focusRun}
            onAgentFinished={() => {
              if (conversationRef.current !== convId) return;
              refreshAgentActivity.current();
              if (convId) void reloadMsgs(convId);
              void refreshConvs();
            }}
            onAgentActiveChange={(active) => { if (active && !agentBusy && conversationRef.current === convId) refreshAgentActivity.current(); }}
          />
        </RightPanel>
      )}

      {guard && (
        <Dialog
          role="alertdialog"
          icon="alert"
          title="Switch models while the agent is working?"
          description={guard.detail}
          onClose={() => setGuard(null)}
          footer={<>
            <Button variant="ghost" onClick={() => setGuard(null)}>Cancel</Button>
            <Button onClick={() => void guardWait()}>Wait for the current step</Button>
            <Button variant="danger" icon="stop" onClick={() => void guardStopSwitch()}>Stop agent and switch</Button>
          </>}
        />
      )}

      {confirmState && (
        <Dialog
          size="sm"
          role="alertdialog"
          icon={confirmState.icon}
          title={confirmState.title}
          description={confirmState.body}
          onClose={() => setConfirmState(null)}
          footer={<>
            <Button variant="ghost" onClick={() => setConfirmState(null)}>Cancel</Button>
            <Button variant="primary" onClick={() => { const run = confirmState.onConfirm; setConfirmState(null); run(); }}>{confirmState.action}</Button>
          </>}
        />
      )}
    </div>
  );
}
