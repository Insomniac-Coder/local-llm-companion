import { useCallback, useEffect, useRef, useState, type ReactNode } from 'react';
import MessageView from './components/MessageView';
import PrepareBanner from './components/PrepareBanner';
import ModelLibraryItem from './components/ModelLibraryItem';
import ResourcesPanel from './components/ResourcesPanel';
import SettingsPanel from './components/SettingsPanel';
import Toasts, { pushToast, type Toast } from './components/Toasts';
import ContextBar from './components/ContextBar';
import DiffModal from './components/DiffModal';
import LoadProgress from './components/LoadProgress';
import ShareDialog, { type ShareOptions } from './components/ShareDialog';
import RecoveryBanner from './components/RecoveryBanner';
import DoctorCard from './components/DoctorCard';
import BenchmarkCard from './components/BenchmarkCard';
import PluginsCard from './components/PluginsCard';
import SetupWizard from './components/SetupWizard';
import CodeHeader from './components/CodeHeader';
import AttachChips from './components/AttachChips';
import SessionMenu from './components/SessionMenu';
import ModelSelect from './components/ModelSelect';
import PermissionsModal from './components/PermissionsModal';
import RightPanel from './components/RightPanel';
import RightPanelTabs from './components/RightPanelTabs';
import ProjectLauncher from './components/ProjectLauncher';
import AgentChatProgress from './components/AgentChatProgress';
import CommandPalette from './components/CommandPalette';
import WorkStatus from './components/WorkStatus';
import { AUTO_POLICY_DESCRIPTION, PROJECT_BOUNDARY_DESCRIPTION, SEARCH_PERMISSION_DESCRIPTION } from './components/permissionCopy';
import { VisibleOutputMeter, type GenerationPhase, type OutputTiming } from './services/outputTiming';
import { applyAgentContext } from './services/contextUsage';
import { currentActivitySnapshot, parseActivityStart, visibleWorkActivity } from './services/workElapsed';
import { type CodeIntent, matchesShortcut, selectAvailableModel, shouldStartAgent, updateMessage, WORKBENCH_DESTINATIONS } from './services/workbench';
import { Badge, Button, PopItem, Popover, Toggle, Tooltip } from './ui/primitives';
import { activityLabel } from './services/events';
import {
  agentRuns, classifyRequest, compactConversation, createConversation, deleteConversation, deleteModel, discardStale, downloadAction, editMessage, exportConversation, forkConversation,
  getContext, getConversationMetrics, getMessages, getOverview, getRecovery, getPermissionMode, getSettings, inferenceStart,
  inferenceStatus, inferenceStop, listCommands, listConversations, listDownloads,
  listModels, listSessions, listTools, listWorkspaces, patchSession, stopAgent,
  loadModel, modelDetail, patchConversation, scanModels, shareConversation,
  setPermissionMode as updatePermissionMode, startAgent, startDownload, stopChat, streamChat, unloadModels,
  systemInfo, uploadAttachment,
  type AgentEvent, type CommandItem, type ContextInfo, type Conversation, type DownloadInfo,
  type InferenceStatus, type ModelMeta, type RecoveryInfo, type SessionInfo,
  type StreamUsage, type PersistedMetric, type ToolDescriptor, type Workspace,
} from './services/api';

type Msg = { id: string; role: 'user' | 'assistant' | 'tool'; text: string; time: string; activities?: AgentEvent[] };

type Theme = 'dark' | 'light' | 'system';

type AppIconName = 'chat' | 'code' | 'models' | 'resources' | 'system' | 'settings' | 'tools';

function AppIcon({ name }: { name: AppIconName }) {
  const paths: Record<AppIconName, ReactNode> = {
    chat: <><path d="M4.5 5.5h15v10h-9l-4 3v-3h-2z" /><path d="M8 9h8M8 12h5" /></>,
    code: <><path d="m8.5 7-5 5 5 5M15.5 7l5 5-5 5M13.5 4l-3 16" /></>,
    models: <><path d="m12 3 8 4-8 4-8-4z" /><path d="m4 11 8 4 8-4M4 15l8 4 8-4" /></>,
    resources: <><path d="M5 19V9M12 19V4M19 19v-7" /><path d="M3 19h18" /></>,
    system: <><rect x="3.5" y="4" width="17" height="16" rx="2" /><path d="M7 9h4M7 13h7M7 17h10" /></>,
    settings: <><path d="M4 7h10M18 7h2M4 17h2M10 17h10M4 12h4M12 12h8" /><circle cx="16" cy="7" r="2" /><circle cx="8" cy="17" r="2" /><circle cx="10" cy="12" r="2" /></>,
    tools: <><path d="M4 5h16v14H4z" /><path d="m8 10 2 2-2 2M13 15h4" /></>,
  };
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      {paths[name]}
    </svg>
  );
}

function applyTheme(t: Theme) {
  const mq = matchMedia('(prefers-color-scheme: light)');
  document.documentElement.dataset.theme = t === 'system' ? (mq.matches ? 'light' : 'dark') : t;
}

export default function App() {
  const [tab, setTab] = useState<'chat' | 'models' | 'resources' | 'system' | 'settings' | 'tools'>('chat');
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
  const [dlId, setDlId] = useState('');
  const [dlUrl, setDlUrl] = useState('');
  const [dlSha, setDlSha] = useState('');
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
  const [showShare, setShowShare] = useState(false);
  const [focusRun, setFocusRun] = useState<string | null>(null);
  const [guard, setGuard] = useState<{ kind: 'load' | 'start'; id: string; detail: string } | null>(null);
  const [sessions, setSessions] = useState<SessionInfo[]>([]);
  const [pill, setPill] = useState('');
  // Stages 21–27 shell state.
  const [collapsed, setCollapsed] = useState(() => typeof window !== 'undefined' && window.matchMedia('(max-width: 850px)').matches);
  const [mobileNav, setMobileNav] = useState(false);
  const [showLatest, setShowLatest] = useState(false);
  const [backendUp, setBackendUp] = useState<boolean | null>(null);
  // UI guide §3: three-zone shell. Right panel content lands in Stage 41.
  const [rightOpen, setRightOpen] = useState(false);
  const [rightTab, setRightTab] = useState(mode === 'code' ? 'activity' : 'context');
  // UI guide §4: pinned sessions, inline rename.
  const [pinnedIds, setPinnedIds] = useState<string[]>(() => {
    try { return JSON.parse(localStorage.getItem('companion.pinned') ?? '[]'); } catch { return []; }
  });
  const [renaming, setRenaming] = useState<{ id: string; draft: string } | null>(null);
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
  const [permissionMode, setPermissionModeState] = useState<'ask' | 'auto'>('ask');
  const [permissionModeBusy, setPermissionModeBusy] = useState(true);
  const [compacting, setCompacting] = useState(false);
  const [diffWs, setDiffWs] = useState<string | null>(null);
  const [recovery, setRecovery] = useState<RecoveryInfo | null>(null);
  const [recoveryOff, setRecoveryOff] = useState(false);
  const [dragOver, setDragOver] = useState(false);
  const [liveTps, setLiveTps] = useState<number | null>(null);
  const [showGenerationSpeed, setShowGenerationSpeed] = useState(true);
  const [showDetailedMetrics, setShowDetailedMetrics] = useState(false);
  const [perfMap, setPerfMap] = useState<Record<string, { tps: number | null; timing?: OutputTiming | null; legacy: boolean }>>({});
  const [generationPhase, setGenerationPhase] = useState<GenerationPhase>('processing');
  const lastTpsPush = useRef(0);
  const composerRef = useRef<HTMLTextAreaElement | null>(null);
  const abort = useRef<AbortController | null>(null);
  const fileRef = useRef<HTMLInputElement | null>(null);
  const conversationRef = useRef<string | null>(convId);
  conversationRef.current = convId;
  const transcriptRef = useRef<HTMLDivElement>(null);
  const followOutput = useRef(true);
  const drafts = useRef<Record<string, string>>({});

  const notify = useCallback((kind: Toast['kind'], text: string) => pushToast(setToasts, kind, text), []);
  const setNotice = (text: string) => notify('info', text);

  useEffect(() => {
    applyTheme(theme);
    localStorage.setItem('companion.theme', theme);
  }, [theme]);

  useEffect(() => {
    const onAppearance = (event: Event) => {
      const value = (event as CustomEvent).detail;
      if (['dark', 'light', 'system'].includes(value?.theme)) setTheme(value.theme);
    };
    window.addEventListener('companion:appearance', onAppearance);
    const onSettings = (event: Event) => {
      const value = (event as CustomEvent).detail;
      setPermissionModeState(value.agent?.autonomous_enabled ? 'auto' : 'ask');
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
      setModels(next);
      // A model can disappear between launches (for example, after its
      // directory is removed or the models folder is reset). Never keep a
      // stale id in the selector: it would make Load/Start send an id the
      // backend can no longer resolve.
      setModelId((current) => selectAvailableModel(next, current, preferredDefaultModel.current));
    } catch { /* backend offline */ }
  }

  async function refreshDownloads() {
    try {
      setDownloads(await listDownloads());
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
      setSessions(r.sessions);
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
    getRecovery().then(setRecovery).catch(() => setRecovery(null));
    const t = setInterval(refreshDownloads, 2000);
    const modelRefresh = setInterval(() => { if (!document.hidden) { void refreshModels(); inferenceStatus().then((state) => { setInf(state); setBackendUp(true); }).catch(() => setBackendUp(false)); } }, 10000);
    systemInfo().then((v) => { setSys(v); setBackendUp(true); }).catch(() => { setSys(null); setBackendUp(false); });
    inferenceStatus().then(setInf).catch(() => setInf(null));
    getPermissionMode().then((result) => {
      setPermissionModeState(result.mode);
      localStorage.setItem('companion.permissionMode', result.mode);
    }).catch(() => notify('warning', 'Could not read the saved approval policy. Reconnect to the local runtime before changing it.'))
      .finally(() => setPermissionModeBusy(false));
    const p = setInterval(() => {
      getOverview()
        .then((o) => {
          const l = o?.resources;
          setPill(l ? `RAM ${l.ram_used_gb?.toFixed?.(1) ?? '?'}G` : '');
        })
        .catch(() => setPill(''));
    }, 8000);
    return () => { clearInterval(t); clearInterval(p); clearInterval(modelRefresh); };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  async function changePermissionMode(next: 'ask' | 'auto') {
    if (permissionModeBusy || next === permissionMode) return;
    const previous = permissionMode;
    setPermissionModeState(next);
    setPermissionModeBusy(true);
    try {
      const result = await updatePermissionMode(next);
      setPermissionModeState(result.mode);
      localStorage.setItem('companion.permissionMode', result.mode);
      notify('success', result.mode === 'auto'
        ? `Auto mode on${result.resumed ? ` — resumed ${result.resumed} waiting task${result.resumed === 1 ? '' : 's'}` : ''}.`
        : 'Ask mode on — agent actions will request approval.');
    } catch (error: any) {
      setPermissionModeState(previous);
      notify('error', error?.message ?? 'Could not change permission mode.');
    } finally {
      setPermissionModeBusy(false);
    }
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
        setAgentActivity(live ? { conversationId: convId, runId: live.id, startedAt: parseActivityStart(live.started_at) } : null);
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
        next[m.message_id] = { tps: m.timing ? m.timing.output_tps : m.gen_tps, timing: m.timing, legacy: !m.timing };
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
    } catch (e: any) {
      notify('error', e?.message ?? 'Could not create conversation (is the backend running?)');
    }
  }

  async function send(override?: string) {
    const raw = override ?? input;
    if (!raw.trim() || busy || agentBusy || (convId && pendingAgentStarts.current.has(convId))) return;
    if (!raw.trim().startsWith('/') && (!inf?.running || models.find((model) => model.loaded)?.id !== modelId)) {
      notify('warning', 'Load the selected model before sending a message. Your draft is kept.');
      return;
    }
    let cid = convId;
    if (!cid) {
      try {
        const extra = mode === 'code' ? { mode, workspace: wsId } : { mode };
        if (mode === 'code' && !workspaces.some((workspace) => workspace.id === wsId)) {
          notify('warning', 'Choose a project first.');
          setProjectLauncherOpen(true);
          return;
        }
        const c = await createConversation(raw.slice(0, 60) || 'New chat', modelId, extra);
        setConvs((prev) => [c, ...prev]);
        cid = c.id;
        conversationRef.current = cid;
        setConvId(cid);
        localStorage.setItem(`companion.last.${mode}`, cid);
      } catch (e: any) {
        setMsgs((m) => [...m, { id: `tmp-${Date.now()}`, role: 'tool', text: `Error: ${e?.message ?? e}`, time: '' }]);
        return;
      }
    }
    const text = raw;
    let inferredIntent: CodeIntent = 'ask';
    let classified = false;
    if (mode === 'code' && !text.trim().startsWith('/')) {
      const routingController = new AbortController();
      abort.current = routingController;
      setBusy(true);
      setChatActivity({conversationId: cid, startedAt: Date.now()});
      setStatusLine('Understanding your request…');
      try {
        const decision = await classifyRequest(text, cid, routingController.signal);
        if (routingController.signal.aborted || conversationRef.current !== cid) return;
        inferredIntent = decision.intent;
        classified = decision.source === 'model';
        if (decision.source === 'fallback') notify('info', 'Could not determine the request type. Continuing in chat.');
      } catch (error: any) {
        if (error?.name !== 'AbortError') notify('error', error?.message ?? 'Could not classify the message. Your draft is kept.');
        return;
      } finally {
        setBusy(false);
        setChatActivity(null);
        setStatusLine('');
      }
    }
    if (override === undefined) {
      setInput('');
      if (composerRef.current) composerRef.current.style.height = 'auto';
    }
    setCmdMenu([]);
    const tmpId = `tmp-${Date.now()}`;
    setMsgs((m) => [...m, { id: tmpId, role: 'user', text, time: '' }]);

    followOutput.current = true;
    if (shouldStartAgent(mode, inferredIntent, text)) {
      const linkedWorkspace = convs.find((conversation) => conversation.id === cid)?.workspace ?? wsId;
      const workspace = workspaces.find((candidate) => candidate.id === linkedWorkspace);
      if (!workspace) {
        setMsgs((messages) => messages.filter((message) => message.id !== tmpId));
        notify('warning', 'Choose a project so the agent has a safe working boundary.');
        setProjectLauncherOpen(true);
        return;
      }
      setAgentBusy(true);
      pendingAgentStarts.current.add(cid);
      setAgentActivity({ conversationId: cid, runId: '', startedAt: null });
      setAgentPhase('ROUTING');
      try {
        const agentMode = inferredIntent === 'plan' ? 'plan' : 'agent';
        const run = await startAgent(workspace.path, text.trim(), agentMode, cid, {search, classified});
        if ('run_id' in run) {
          if (conversationRef.current === cid) {
            setFocusRun(run.run_id);
            setAgentPhase('PLANNING');
            setRightOpen(true);
            setRightTab('activity');
          }
          void getContext(cid).then((context) => { if (conversationRef.current === cid) setCtx(context); }).catch(() => {});
          notify('success', 'Work started. Progress and approvals are in Session Activity.');
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
      }
      return;
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
            const entry = { tps: u.timing ? u.timing.output_tps : u.gen_tps ?? null, timing: u.timing, legacy: !u.timing };
            setPerfMap((p) => ({ ...p, [u.message_id ?? `${tmpId}-a`]: entry }));
          }
          const cmd = u.command;
          if (cmd?.type === 'clear' && cmd.conversation_id) {
            refreshConvs(cmd.conversation_id);
          } else if (cmd?.type === 'retry' && cmd.text) {
            void send(cmd.text);
          } else if (cmd?.type === 'agent' && cmd.run_id) {
            setFocusRun(cmd.run_id);
            setRightOpen(true);
            setRightTab('activity');
            notify('success', `Agent run started (${cmd.run_id.slice(0, 8)}).`);
          }
          if (cid) void maybeAutoTitle(cid, text);
        },
        onError: (msg) => { if (conversationRef.current === cid) setMsgs((m) => [...m, { id: `tmp-${Date.now()}`, role: 'tool', text: `Error: ${msg}`, time: '' }]); },
      }, ctl.signal, { reasoning, search, classified });
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
    }
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
    if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); void send(); }
  }

  async function doFork(id?: string) {
    const target = id ?? convId;
    if (!target) return;
    try {
      const r = await forkConversation(target);
      notify('success', `Forked (${r.messages} messages).`);
      refreshConvs(r.forked);
      refreshSessions();
    } catch (e: any) {
      notify('error', e?.message ?? 'Fork failed.');
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
      notify('success', 'Session exported.');
    } catch (e: any) {
      notify('error', e?.message ?? 'Export failed.');
    }
  }

  // UI guide §4: hover menu actions + auto-titles ("RageV — Metallic Ghosting").
  function togglePin(id: string) {
    setPinnedIds((prev) => {
      const next = prev.includes(id) ? prev.filter((p) => p !== id) : [...prev, id];
      localStorage.setItem('companion.pinned', JSON.stringify(next));
      return next;
    });
  }

  async function doRename(id: string, title: string) {
    const t = title.trim().slice(0, 60);
    if (!t) return;
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
    const cur = convs.find((c) => c.id === cid);
    if (!cur || (cur.title !== 'New chat' && cur.title !== 'New Chat')) return;
    const t = userText.replace(/^\/\w+\s*/, '').trim().slice(0, 48);
    if (!t) return;
    try {
      await patchConversation(cid, { title: t });
      refreshConvs();
    } catch { /* title is cosmetic; never fail the turn */ }
  }

  async function doDelete(id: string) {
    if (!confirm('Delete this conversation? History will be removed.')) return;
    try {
      await deleteConversation(id);
      if (convId === id) { setConvId(null); setMsgs([]); setCtx(null); }
      refreshConvs();
      refreshSessions();
    } catch (e: any) {
      notify('error', e?.message ?? 'Delete failed.');
    }
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

  const visibleConvs = convs.filter((c) => (c.mode || 'chat') === mode && c.title.toLowerCase().includes(sessionFilter.toLowerCase()));
  const loadedModel = models.find((m) => m.loaded)?.id ?? '';
  const activeConv = convs.find((c) => c.id === convId) ?? null;
  const needsPrepare = !!(
    activeConv?.last_model &&
    loadedModel &&
    activeConv.last_model !== loadedModel
  );

  /** Load/start with the agent-execution guard (§178). */
  async function guardedSwitch(kind: 'load' | 'start', id: string, force: boolean) {
    setLoadingModel(true);
    try {
      if (kind === 'load') {
        await loadModel(id, force);
      } else {
        await inferenceStart(id, force);
      }
      await refreshModels();
      const status = await inferenceStatus();
      setInf(status);
      if (!force) notify('success', kind === 'load' ? `${id} is ready.` : 'Inference started.');
      if (status.runtime_notice) notify('info', status.runtime_notice);
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
      notify('info', 'Model unloaded.');
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

  const MODE_ITEMS = [
    { id: 'chat', label: 'Chat', icon: 'chat' },
    { id: 'code', label: 'Code', icon: 'code' },
  ] as const;
  const statusBadge = loadingModel
    ? { tone: 'info' as const, label: 'Loading' }
    : guard
      ? { tone: 'warn' as const, label: 'Switching' }
      : inf?.running
          ? { tone: 'ok' as const, label: 'Ready' }
          : backendUp === false
            ? { tone: 'err' as const, label: 'Error' }
            : { tone: 'neutral' as const, label: 'Standby' };
  const pinnedConvs = pinnedIds.map((id) => convs.find((c) => c.id === id)).filter((c) => c != null);
  const visibleWork = visibleWorkActivity(convId, busy, chatActivity, agentBusy, agentActivity);

  return (
    <div className={`shell${collapsed ? ' rail' : ''}${rightOpen && tab === 'chat' ? '' : ' no-right'}${mobileNav ? ' mobile-nav' : ''}`}>
      {paletteOpen && <CommandPalette onClose={() => setPaletteOpen(false)} actions={[
        {id:'new', label: mode === 'code' ? 'New code session' : 'New conversation', detail:'Start fresh', run: () => void newChat()},
        ...WORKBENCH_DESTINATIONS.map(({id, label}) => ({id, label, detail:'Open workspace view', run: () => setTab(id)})),
        {id:'project',label:'Open a project',detail:'Choose the files your coding agent can access',run: () => setProjectLauncherOpen(true)},
        {id:'panel',label:'Toggle split view',detail:'Activity, files and context alongside your conversation',run: () => setRightOpen((open) => !open)},
        ...convs.map((conversation) => ({id:conversation.id,label:conversation.title,detail:conversation.mode === 'code' ? 'Code session' : 'Conversation',run: () => void selectConv(conversation.id)})),
      ]} />}
      {mobileNav && <button className="nav-scrim" aria-label="Close navigation" onClick={() => setMobileNav(false)} />}
      <a href="#composer" className="skip-link" onClick={(e) => { e.preventDefault(); composerRef.current?.focus(); }}>
        Skip to composer
      </a>
      <aside className="sidebar" aria-label="Navigation">
        <div className="brandline">
          <strong className="brand rail-hide">
            <i className="brand-signal" aria-hidden="true" />
            <span className="brand-copy">Companion<small>Local runtime</small></span>
          </strong>
          <Tooltip tip={mode === 'code' ? 'New code chat' : 'New chat'}>
            <Button variant="ghost" size="sm" className="rail-only" style={{ display: 'none' }} onClick={() => void newChat()} aria-label={mode === 'code' ? 'New code chat' : 'New chat'}>
              +
            </Button>
          </Tooltip>
          <span style={{ flex: 1 }} />
          <Tooltip tip={collapsed ? 'Expand sidebar' : 'Collapse sidebar'}>
            <button className="ctx-toggle sidebar-collapse" onClick={() => {
              if (window.matchMedia('(max-width: 850px)').matches) setMobileNav(false);
              else setCollapsed((v) => !v);
            }} aria-label={collapsed ? 'Expand sidebar' : 'Collapse sidebar'}>
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
                <path d={collapsed ? 'm9 6 6 6-6 6' : 'm15 6-6 6 6 6'} />
                <path d={collapsed ? 'M4 5v14' : 'M20 5v14'} opacity=".45" />
              </svg>
            </button>
          </Tooltip>
        </div>
        <div className="rail-hide">
          <Button variant="primary" className="new-session" onClick={() => void newChat()}><span aria-hidden="true">+</span> New {mode === 'code' ? 'task' : 'chat'}</Button>
          <button className="quick-search" onClick={() => setPaletteOpen(true)}><span>Find anything</span><kbd>{paletteShortcut.replace(/\+/g, ' ')}</kbd></button>
        </div>
        {mode === 'code' && (
          <div className="workspace-switcher rail-hide">
            <div className="sidebar-section-label">Project</div>
            <select aria-label="Active project" disabled={busy || agentBusy} value={workspaces.some((workspace) => workspace.id === wsId) ? wsId : ''} onChange={(e) => void changeWorkspace(e.target.value)} style={{ width: '100%', marginTop: 4 }}>
              <option value="">— select project —</option>
              {workspaces.map((w) => (
                <option key={w.id} value={w.id}>{w.name}</option>
              ))}
            </select>
            <button className="open-project" onClick={() => setProjectLauncherOpen(true)}>Open or create project…</button>
          </div>
        )}
        <div className="sidebar-section-label rail-hide">Work</div>
        <nav aria-label="Primary" className="primary-nav">
          {MODE_ITEMS.map((t) => (
            <Tooltip key={t.id} tip={t.label}>
              <button
                className={mode === t.id && tab === 'chat' ? 'active' : ''}
                aria-label={t.label}
                onClick={() => switchMode(t.id)}
                aria-current={mode === t.id && tab === 'chat' ? 'page' : undefined}
                style={collapsed ? { textAlign: 'center' } : undefined}
              >
                <span className="nav-glyph"><AppIcon name={t.icon} /></span>
                <span className="rail-hide">{t.label}</span>
              </button>
            </Tooltip>
          ))}
        </nav>
        {pinnedConvs.length > 0 && (
          <div className="rail-hide">
            <div className="sidebar-section-label">Pinned</div>
            <nav className="convlist" aria-label="Pinned sessions">
              {pinnedConvs.map((c) => (
                <div key={c!.id} className={`convrow${convId === c!.id ? ' active' : ''}`}>
                  <button className="convbtn" onClick={() => void selectConv(c!.id)} title={c!.title}>
                    📌 {(c!.title || 'Untitled').slice(0, 24)}
                  </button>
                  <button className="ctx-toggle" onClick={() => togglePin(c!.id)} aria-label={`Unpin ${c!.title}`} title="Unpin">
                    ×
                  </button>
                </div>
              ))}
            </nav>
          </div>
        )}
        <div className="sidebar-section-label rail-hide">
          {mode === 'code' ? 'Code sessions' : 'Conversations'}
        </div>
        <input className="session-filter rail-hide" aria-label="Filter sessions" placeholder="Filter sessions…" value={sessionFilter} onChange={(event) => setSessionFilter(event.target.value)} />
        <nav className="convlist" aria-label={mode === 'code' ? 'Code sessions' : 'Conversations'}>
          {visibleConvs.map((c) => {
            const sess = sessions.find((sn) => sn.id === c.id);
            const dot = sess?.residency === 'active' ? '● ' : sess?.residency === 'warm' ? '◐ ' : '';
            const act = activityLabel(sess?.activity);
            if (renaming?.id === c.id) {
              return (
                <div key={c.id} className="convrow">
                  <input
                    className="rename-input"
                    autoFocus
                    value={renaming.draft}
                    onChange={(e) => setRenaming({ id: c.id, draft: e.target.value })}
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
            return (
              <div key={c.id} className={`convrow${convId === c.id ? ' active' : ''}`}>
                <button
                  className={`convbtn${collapsed ? ' rail-hide' : ''}`}
                  onClick={() => void selectConv(c.id)}
                  title={sess ? `${act} · residency: ${sess.residency} · priority: ${sess.priority ?? 'normal'}` : ''}
                >
                  {dot}{c.title || 'Untitled'}
                </button>
                <SessionMenu
                  pinned={pinnedIds.includes(c.id)}
                  onRename={() => setRenaming({ id: c.id, draft: c.title || '' })}
                  onDuplicate={() => { void selectConv(c.id); setTimeout(() => void doFork(c.id), 50); }}
                  onTogglePin={() => togglePin(c.id)}
                  onExport={() => void doExport(c.id)}
                  onClose={() => void doDelete(c.id)}
                />
              </div>
            );
          })}
          {visibleConvs.length === 0 && <div className="rail-hide" style={{ fontSize: 12, color: 'var(--text-muted)' }}>None yet — start one above.</div>}
        </nav>
        <nav className="utility-nav" aria-label="Manage Companion">
          {WORKBENCH_DESTINATIONS.map(({id, label}) => <button key={id} className={tab === id ? 'active' : ''} title={label} aria-label={label} aria-current={tab === id ? 'page' : undefined} onClick={() => { setTab(id); setMobileNav(false); }}><span className="nav-glyph"><AppIcon name={id}/></span><span className="rail-hide">{label}</span></button>)}
        </nav>
        <div className="sidebar-footer rail-hide">
          <span className={backendUp === false ? 'offline' : ''} />
          {backendUp === false ? 'Runtime offline' : 'Private on this PC'}
        </div>
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
      <div className="center">
        <div className="topbar">
          <button className="mobile-menu" onClick={() => setMobileNav((v) => !v)} aria-label="Open navigation" aria-expanded={mobileNav}>☰</button>
          <div className="topbar-title">
            <strong>{tab !== 'chat' ? ({models:'Model library',resources:'Resources',system:'Runtime & diagnostics',tools:'Tools & plugins',settings:'Settings'})[tab] : activeConv?.title || (mode === 'code' ? 'Code workspace' : 'New conversation')}</strong>
            <span>{tab !== 'chat' ? 'Your local environment' : mode === 'code' ? `Code / ${workspaces.find((workspace) => workspace.id === (activeConv?.workspace ?? wsId))?.name ?? 'choose a project'}` : 'Chat / Local inference'}</span>
          </div>
          <div className="topbar-model">
          <ModelSelect
            models={models}
            modelId={modelId}
            running={!!inf?.running}
            busy={loadingModel}
            onSelect={setModelId}
            onLoad={() => void guardedSwitch('load', modelId, false)}
            onUnload={() => void unloadSelectedModel()}
            onReload={() => void reloadSelectedModel()}
          />
          <Badge tone={statusBadge.tone} title={inf?.model ? `Model: ${inf.model}` : 'Inference state'}>{statusBadge.label}</Badge>
          </div>
          <div className="topbar-actions">
          {pill && <span className="respill">{pill}</span>}
          <span style={{ position: 'relative' }}>
            <Tooltip tip="Session actions">
              <button onClick={() => setSessionMenuOpen((v) => !v)} aria-haspopup="menu" aria-expanded={sessionMenuOpen} aria-label="Session actions">
                ⋯
              </button>
            </Tooltip>
            <Popover open={sessionMenuOpen} onClose={() => setSessionMenuOpen(false)} label="Session actions">
              <PopItem onClick={() => { setSessionMenuOpen(false); doClear(); }}>Clear session</PopItem>
              <PopItem onClick={() => { setSessionMenuOpen(false); void doCompact(); }}>Compact context</PopItem>
              <PopItem onClick={() => { setSessionMenuOpen(false); void doFork(); }}>Fork</PopItem>
              <PopItem onClick={() => { setSessionMenuOpen(false); setShowShare(true); }}>Share…</PopItem>
              <PopItem onClick={() => { setSessionMenuOpen(false); void doExport(); }}>Export</PopItem>
              <PopItem onClick={() => {
                setSessionMenuOpen(false);
                if (convId) setRenaming({ id: convId, draft: activeConv?.title ?? '' });
              }}>
                Rename
              </PopItem>
              <PopItem onClick={() => { setSessionMenuOpen(false); setPermOpen(true); }}>Permissions…</PopItem>
            </Popover>
          </span>
          <Tooltip tip={`Theme: ${theme} (click to change)`}>
            <button onClick={() => setTheme(theme === 'dark' ? 'light' : theme === 'light' ? 'system' : 'dark')} aria-label={`Theme: ${theme}. Activate to change.`}>
              {theme === 'dark' ? '☾' : theme === 'light' ? '☀' : '◐'}
            </button>
          </Tooltip>
          <Tooltip tip={rightOpen ? 'Close session panel' : mode === 'code' ? 'Open session activity' : 'Open session context'}>
            <button onClick={() => setRightOpen((v) => !v)} aria-label={rightOpen ? 'Close session panel' : 'Open session panel'} aria-expanded={rightOpen}>
              ◫
            </button>
          </Tooltip>
          </div>
        </div>
        {permOpen && (
          <PermissionsModal
            onClose={() => setPermOpen(false)}
            onOpenSettings={() => setTab('settings')}
          />
        )}
        <Toasts toasts={toasts} dismiss={(id) => setToasts((p) => p.filter((t) => t.id !== id))} />

        {backendUp === false && (
          <div className="card" role="alert" style={{ margin: '8px 16px 0', borderColor: 'var(--error)' }}>
            <strong>Local runtime unavailable.</strong>{' '}
            <span style={{ fontSize: 13 }}>
              Close this window and start Companion again with <code>.\run.ps1</code>.
            </span>
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
              notify('info', 'Stale marker cleared.');
              getRecovery().then(setRecovery).catch(() => {});
              refreshConvs();
            }).catch((e) => notify('error', e.message))}
            onDismiss={() => setRecoveryOff(true)}
          />
        )}

        <LoadProgress active={loadingModel} notify={notify} />

        {guard && (
          <div className="card" role="alertdialog" aria-label="Model switch requested" style={{ borderColor: 'var(--warning)', margin: '8px 16px 0' }}>
            <div><strong>Model switch requested</strong></div>
            <div style={{ fontSize: 13 }}>{guard.detail}</div>
            <div style={{ display: 'flex', gap: 8, marginTop: 8 }}>
              <button onClick={() => void guardWait()}>Wait for Current Step</button>
              <button onClick={() => void guardStopSwitch()}>Stop Agent and Switch</button>
              <button onClick={() => setGuard(null)}>Cancel</button>
            </div>
          </div>
        )}

        {tab === 'chat' && needsPrepare && activeConv && (
          <PrepareBanner
            convId={activeConv.id}
            convTitle={activeConv.title}
            lastModel={activeConv.last_model ?? ''}
            loadedModel={loadedModel}
            lastModelAvailable={models.some((model) => model.id === activeConv.last_model)}
            onPrepared={() => refreshConvs(activeConv.id)}
            onSwitchBack={() => {
              const previous = activeConv.last_model ?? '';
              if (!models.some((model) => model.id === previous)) {
                notify('warning', `The previous model '${previous}' is no longer installed. Prepare this session for ${loadedModel} instead.`);
                return;
              }
              void guardedSwitch('load', previous, false);
            }}
            notify={(k, t) => (k === 'error' ? notify('error', t) : notify(k === 'success' ? 'success' : 'info', t))}
          />
        )}

        {tab === 'chat' && (
          <>
            {mode === 'code' && activeConv?.workspace && (
              <CodeHeader
                wsId={activeConv.workspace}
                workspaces={workspaces}
                busy={busy || agentBusy}
                onChanges={() => setDiffWs(activeConv.workspace!)}
                onCommand={(command) => { if (command === '/run ') { setInput(command); composerRef.current?.focus(); } else void send(command); }}
              />
            )}
            <div className="contextbar">
              {agentBusy ? inf?.running ? 'Agent working in this project…' : 'Run needs attention · model runtime unavailable' : busy ? (statusLine || 'Generating…') : mode === 'code' ? 'Ask a question, request a plan, or describe a change' : 'History stays on this computer'}
              {usage ? ` · last turn: ${usage.prompt_tokens} prompt / ${usage.generated_tokens} generated${usage.stopped ? ' · stopped, partial kept' : ''}${usage.reasoning && usage.reasoning !== 'off' ? ` · reasoned (${usage.reasoning})` : ''}${usage.sources ? ` · ${usage.sources} sources` : ''}${usage.vision === 'unsupported' ? ' · images skipped (no vision)' : ''}${usage.tool_rounds ? ` · read ${usage.tool_rounds} file${usage.tool_rounds === 1 ? '' : 's'} in-chat` : ''}` : ''}
              <span className="spacer" />
              {convId && (
                <select
                  value={sessions.find((sn) => sn.id === convId)?.priority ?? 'normal'}
                  onChange={(e) => void setPriority(e.target.value)}
                  title="Scheduling priority (§138)"
                  aria-label="Scheduling priority"
                >
                  <option value="background">Background</option>
                  <option value="normal">Normal</option>
                  <option value="high">High</option>
                </select>
              )}
            </div>
            <ContextBar ctx={ctx} onCompact={() => void doCompact()} compacting={compacting} />
            {showShare && convId && (
              <ShareDialog
                targets={convs.filter((c) => c.id !== convId).map((c) => ({ id: c.id, label: `${c.mode === 'code' ? '💻' : '💬'} ${c.title}` }))}
                onShare={(t, o) => void doShare(t, o)}
                onClose={() => setShowShare(false)}
              />
            )}
            {historyLoading ? <div className="chat history-loading" role="status">Opening conversation…</div> : msgs.length === 0 ? (
              <div className="chat"><div className="empty welcome">
                <div className="welcome-mark" aria-hidden="true"><AppIcon name={mode === 'code' ? 'code' : 'chat'} /></div>
                <h1>{mode === 'code' ? 'A little idea. A working project.' : 'Room for your next idea.'}</h1>
                <p>{mode === 'code' ? 'Explore your code, work through a plan, or give the agent a change to make. You choose how it works.' : 'Think it through, write something useful, or make sense of a file. Your model, on your machine.'}</p>
                <div className="starter-grid">
                  {(mode === 'code' ? [
                    {label:'Understand this project', text:'Explore the project and explain its purpose, main components, and how to run it.', intent:'ask'},
                    {label:'Plan a change', text:'Help me plan a change to this project. First, ask what I want to achieve.', intent:'ask'},
                    {label:'Find a bug', text:'Inspect this project for a concrete bug. Show the evidence and suggest a fix without modifying files.', intent:'ask'},
                  ] : [
                    {label:'Think it through',text:'Help me think through an idea. Ask me what I am trying to achieve.',intent:'ask'},
                    {label:'Draft something',text:'Help me write a clear first draft. Ask me about the audience and purpose.',intent:'ask'},
                    {label:'Explain a concept',text:'Help me understand a concept step by step. Ask me which topic.',intent:'ask'},
                  ]).map((starter) => <button key={starter.label} onClick={() => { setInput(starter.text); composerRef.current?.focus(); }}>{starter.label}<span aria-hidden="true">↗</span></button>)}
                </div>
                {mode === 'code' && !workspaces.some((workspace) => workspace.id === wsId) && <button className="choose-project" onClick={() => setProjectLauncherOpen(true)}>Choose a project to begin</button>}
              </div></div>
            ) : (
              <div className="chat transcript" ref={transcriptRef} onScroll={(event) => { const view = event.currentTarget; followOutput.current = view.scrollHeight - view.scrollTop - view.clientHeight < 100; setShowLatest(!followOutput.current); }}>
                {msgs.length > msgLimit && (
                  <button className="ctx-toggle" onClick={() => setMsgLimit((l) => l + 200)}>
                    Show {msgs.length - msgLimit} older message(s) — rendering newest {msgLimit} for speed
                  </button>
                )}
                {msgs.slice(-msgLimit).filter((message) => !(agentBusy && message.id === focusRun)).map((m) => (
                  editing && editing.mid === m.id ? (
                    <div key={m.id} className={`msg ${m.role}`}>
                      <textarea
                        value={editing.draft}
                        onChange={(e) => setEditing({ mid: m.id, draft: e.target.value })}
                        rows={4}
                        style={{ width: '100%' }}
                      />
                      <div className="msg-actions">
                        <button onClick={() => void saveEdit()}>Save (truncates later turns)</button>
                        <button onClick={() => setEditing(null)}>Cancel</button>
                      </div>
                    </div>
                  ) : (
                    <div key={m.id} className={`message-row ${m.role}`}>
                      <MessageView
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
                        agentRunId={m.id}
                        onOpenAgentActivity={(runId) => {
                          setFocusRun(runId);
                          setRightTab('activity');
                          setRightOpen(true);
                        }}
                      />
                      {!busy && m.role === 'user' && !m.id.startsWith('tmp-') && (
                        <div className="msg-actions">
                          <button onClick={() => setEditing({ mid: m.id, draft: m.text })}>Edit</button>
                        </div>
                      )}
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
                  />
                )}
              </div>
            )}
            {showLatest && <button className="jump-latest" onClick={() => { followOutput.current = true; transcriptRef.current?.scrollTo({top:transcriptRef.current.scrollHeight,behavior:'smooth'}); setShowLatest(false); }}>↓ Latest response</button>}
            <AttachChips convId={convId} tick={attachTick} notify={(k, t) => notify(k === 'error' ? 'error' : 'info', t)} />
            <WorkStatus active={!!visibleWork} startedAt={visibleWork?.startedAt} waiting={visibleWork?.kind === 'agent' && (agentPhase === 'WAITING_PERMISSION' || !inf?.running)} label={visibleWork?.kind === 'agent'
              ? agentPhase === 'ROUTING' ? 'Understanding your request…' : !inf?.running ? 'Model stopped · this run needs attention' : agentPhase === 'WAITING_PERMISSION' ? 'Waiting for your approval' : agentPhase === 'EXECUTING_TOOL' ? 'Working through the project…' : agentPhase === 'OBSERVING' ? 'Reviewing the results…' : 'Planning the next step…'
              : statusLine || (generationPhase === 'thinking' ? 'Companion is thinking…' : generationPhase === 'responding' ? 'Companion is writing…' : 'Companion is processing your request…')} />
            <div
              className={`composer${dragOver ? ' dragover' : ''}`}
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
              <div className="composer-input-row">
                <textarea
                  id="composer"
                  ref={composerRef}
                  value={input}
                  rows={1}
                  onChange={(e) => {
                    void onInput(e.target.value);
                    const el = e.target;
                    el.style.height = 'auto';
                    el.style.height = `${Math.min(el.scrollHeight, 160)}px`;
                  }}
                  onKeyDown={composerKey}
                  placeholder={mode === 'code' ? 'Ask, plan, or describe a change…' : 'Ask anything, or drop a file…'}
                  aria-label="Message composer"
                  disabled={loadingModel || agentBusy}
                />
                {agentBusy ? <button className="send-button stop-button" aria-label="Stop agent" onClick={() => { void agentRuns().then(async (runs) => { await Promise.all(runs.filter((run) => run.conversation_id === convId && !['COMPLETED','FAILED','CANCELLED'].includes(run.state)).map((run) => stopAgent(run.id))); if (conversationRef.current === convId) refreshAgentActivity.current(); if (convId) void reloadMsgs(convId); }).catch((error) => notify('error', error.message)); }}><span aria-hidden="true"/></button> : busy
                  ? <button className="send-button stop-button" onClick={() => {
                      abort.current?.abort();
                      void stopChat()
                        .then((o) => {
                          if (o.stopped) notify('info', `Stopped — partial response kept (${o.chars_kept} chars).`);
                        })
                        .catch(() => { /* fetch abort still applies */ });
                    }} aria-label="Stop generation"><span aria-hidden="true" /></button>
                  : <button className={`send-button${agentBusy ? ' agent-live' : ''}`} onClick={() => void send()} disabled={loadingModel || agentBusy || !input.trim()} title={loadingModel ? 'Model is loading' : agentBusy ? 'Agent is working' : 'Send'} aria-label="Send message">{agentBusy ? '•' : '↑'}</button>}
              </div>
              <div className="composer-toolbar">
                <div className="composer-tools">
                  <input
                    ref={fileRef}
                    type="file"
                    style={{ display: 'none' }}
                    onChange={(e) => { void attach(e.target.files?.[0]); e.target.value = ''; }}
                  />
                  <button disabled={!convId || busy || agentBusy} onClick={() => fileRef.current?.click()} title="Attach a file (text, image, PDF, Office)" aria-label="Attach a file">
                    <span aria-hidden="true">＋</span> Attach
                  </button>
                  {mode === 'code' && (
                    <span className={`permission-mode ${permissionMode}`} role="group" aria-label="Agent approval policy" title={`${AUTO_POLICY_DESCRIPTION} ${PROJECT_BOUNDARY_DESCRIPTION} ${SEARCH_PERMISSION_DESCRIPTION}`}>
                      <button type="button" disabled={permissionModeBusy || busy || agentBusy} className={permissionMode === 'ask' ? 'active' : ''} aria-pressed={permissionMode === 'ask'} onClick={() => void changePermissionMode('ask')}>Ask</button>
                      <button type="button" disabled={permissionModeBusy || busy || agentBusy} className={permissionMode === 'auto' ? 'active' : ''} aria-pressed={permissionMode === 'auto'} onClick={() => void changePermissionMode('auto')}>Auto</button>
                    </span>
                  )}
                  <button
                    className={`toggle${reasoning ? ' on' : ''}`}
                    disabled={busy || agentBusy}
                    title={reasoningCapable ? 'Reasoning: extended response budget. Native thinking depends on the model and runtime.' : 'Extended response budget; native reasoning capability is unverified for this model.'}
                    aria-pressed={reasoning}
                    onClick={() => setReasoning((v) => !v)}
                  >
                    <span className="toggle-dot" /> Reasoning
                  </button>
                  <button className={`toggle${search ? ' on' : ''}`} disabled={busy || agentBusy} aria-pressed={search} title="Search Web: explicit internet access for this message" onClick={() => setSearch((v) => !v)}>
                    <span className="toggle-dot" /> Search
                  </button>
                </div>
                <button className="composer-regenerate" disabled={busy || agentBusy || msgs.length === 0} onClick={regenerate} title="Re-send the last turn">Regenerate</button>
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
            {search && <div className="privacy-note">🌐 Web access enabled for this message — queries go to the configured search provider.</div>}
          </>
        )}

        {tab === 'models' && (
          <div className="chat model-library">
            <div className="page-heading"><div><h1>Your local intelligence.</h1><p>Choose the right model, inspect its capabilities, and tune it for your machine.</p></div><Badge>{models.length} installed</Badge></div>
            <SetupWizard notify={notify} />
            <div className="card">
              <button onClick={() => scanModels().then((r) => {
                notify('success', `Scan: ${r.registered} registered, ${r.warnings.length} warnings.`);
                return refreshModels();
              }).catch((e) => notify('error', e.message))}>
                Scan models/ directory
              </button>
              <div style={{ fontSize: 12, color: 'var(--text-secondary)', marginTop: 6 }}>
                Finds GGUF files in your models folder and its subfolders. Model metadata is optional.
              </div>
            </div>
            <div className="card">
              <strong>Download a model</strong>
              <div style={{ display: 'flex', gap: 8, marginTop: 8, flexWrap: 'wrap' }}>
                <input value={dlId} onChange={(e) => setDlId(e.target.value)} placeholder="id (e.g. qwen-14b)" style={{ flex: 1 }} />
                <input value={dlUrl} onChange={(e) => setDlUrl(e.target.value)} placeholder="https://…/model.gguf" style={{ flex: 2 }} />
                <input value={dlSha} onChange={(e) => setDlSha(e.target.value)} placeholder="sha256 (optional)" style={{ flex: 2 }} />
                <button onClick={() => startDownload(dlId.trim(), dlUrl.trim(), dlSha.trim() || undefined)
                  .then(() => { notify('success', `Download '${dlId.trim()}' started.`); setDlId(''); setDlUrl(''); setDlSha(''); refreshDownloads(); })
                  .catch((e) => notify('error', e.message))}>
                  Download
                </button>
              </div>
              <div style={{ fontSize: 12, color: 'var(--text-secondary)', marginTop: 6 }}>
                Paste a direct GGUF link (e.g. a HuggingFace resolve URL). Resumes from `.part` after interruption.
              </div>
            </div>
            {downloads.length > 0 && (
              <div className="card">
                <strong>Downloads</strong>
                {downloads.map((d) => (
                  <div key={d.id} style={{ marginTop: 8 }}>
                    <div>{d.id} · {d.status}{d.total_bytes ? ` · ${(d.downloaded_bytes / 1048576).toFixed(1)}/${(d.total_bytes / 1048576).toFixed(1)} MB` : ` · ${(d.downloaded_bytes / 1048576).toFixed(1)} MB`}</div>
                    <progress value={d.downloaded_bytes} max={d.total_bytes ?? (d.downloaded_bytes || 1)} style={{ width: '100%' }} />
                    {d.error && <div className="approval">{d.error}</div>}
                    <div style={{ display: 'flex', gap: 8, marginTop: 4 }}>
                      {d.status === 'downloading' && <button onClick={() => downloadAction(d.id, 'pause').then(refreshDownloads).catch((e) => notify('error', e.message))}>Pause</button>}
                      {(d.status === 'paused' || d.status === 'failed' || d.status === 'cancelled') && <button onClick={() => downloadAction(d.id, 'resume').then(refreshDownloads).catch((e) => notify('error', e.message))}>Resume</button>}
                      {(d.status === 'downloading' || d.status === 'paused') && <button onClick={() => downloadAction(d.id, 'cancel').then(refreshDownloads).catch((e) => notify('error', e.message))}>Cancel</button>}
                    </div>
                  </div>
                ))}
              </div>
            )}
            {models.length === 0 && <div className="card">No models registered yet.</div>}
            {models.map((m) => (
              <ModelLibraryItem key={m.id} model={m} loadingModel={loadingModel} notify={notify}
                onLoad={() => { setModelId(m.id); void guardedSwitch('load', m.id, false); }}
                onDelete={() => { if (confirm(`Delete model '${m.id}'? This removes the GGUF permanently.`)) deleteModel(m.id).then(() => { notify('success', `Deleted ${m.id}.`); refreshModels(); }).catch((e) => notify('error', e.message)); }}
              />
            ))}
          </div>
        )}

        {tab === 'resources' && <ResourcesPanel notify={notify} />}

        {tab === 'system' && (
          <div className="chat">
            <div className="page-heading"><div><h1>Under the hood.</h1><p>Check the runtime, diagnose problems, and measure your model’s performance.</p></div></div>
            <div className="card">
              <div><strong>Inference:</strong> {inf ? `${inf.engine}${inf.running ? ` · running ${inf.base_url ?? ''}` : ' · idle'}` : 'backend offline'}</div>
              {inf && <div style={{ fontSize: 12, color: 'var(--text-secondary)' }}>
                Model: {inf.model ?? '—'} · ctx {inf.context_size} · binary: {inf.binary_found ? 'found' : 'missing'}
              </div>}
              {inf?.last_error && <div className="approval">{inf.last_error}</div>}
              <div style={{ display: 'flex', gap: 8, marginTop: 8 }}>
                <button disabled={!modelId} onClick={() => void guardedSwitch('start', modelId, false)}>
                  Start inference
                </button>
                <button onClick={() => inferenceStop().then(() => inferenceStatus().then(setInf)).catch((e) => notify('error', e.message))}>
                  Stop
                </button>
                <button onClick={() => { inferenceStatus().then(setInf); systemInfo().then(setSys); }}>
                  Refresh
                </button>
              </div>
              {!inf?.binary_found && <div style={{ fontSize: 12, color: 'var(--text-secondary)', marginTop: 6 }}>
                No llama-server binary. Download a llama.cpp release, put llama-server(.exe) on PATH or models/bin/, or set COMPANION_LLAMA_SERVER_BIN.
              </div>}
            </div>
            <details className="card"><summary>Hardware and runtime details</summary><pre>{JSON.stringify(sys ?? { hint: 'backend offline' }, null, 2)}</pre></details>
            <DoctorCard notify={notify} />
            <BenchmarkCard notify={notify} />
          </div>
        )}

        {tab === 'tools' && (
          <div className="chat">
            <div className="page-heading"><div><h1>What Companion can do.</h1><p>Inspect available actions and extensions. Tool execution follows your approval policy.</p></div></div>
            <div className="card">
              <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
                <strong>Tool registry</strong>
                <span style={{ flex: 1 }} />
                <button className="ctx-toggle" onClick={() => void refreshRegistry()}>Refresh</button>
              </div>
              <div style={{ fontSize: 12, color: 'var(--text-secondary)', marginTop: 4 }}>
                Same permission gate as the agent — tools never bypass approvals.
              </div>
              {registry.map((t) => (
                <div key={t.name} style={{ display: 'flex', gap: 8, marginTop: 6, fontSize: 13, alignItems: 'baseline' }}>
                  <code style={{ minWidth: 140 }}>{t.name}</code>
                  <Badge tone={t.risk.toLowerCase().startsWith('danger') ? 'err' : t.risk.toLowerCase().startsWith('moderate') ? 'warn' : 'ok'}>
                    {t.risk}
                  </Badge>
                  <span style={{ color: 'var(--text-secondary)', fontSize: 12 }}>{t.description}</span>
                </div>
              ))}
              {registry.length === 0 && <div style={{ fontSize: 13 }}>No tools reported — is the backend running?</div>}
            </div>
            <PluginsCard wsId={wsId} notify={notify} />
          </div>
        )}

        {tab === 'settings' && <SettingsPanel setToasts={setToasts} />}
        {diffWs && <DiffModal wsId={diffWs} onClose={() => setDiffWs(null)} />}
      </div>
      {rightOpen && tab === 'chat' && (
        <RightPanel mode={mode} tab={rightTab} onTab={setRightTab} onClose={() => setRightOpen(false)}>
          <RightPanelTabs
            tab={rightTab}
            convId={convId}
            wsId={activeConv?.workspace ?? ''}
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
    </div>
  );
}
