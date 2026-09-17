import { MODES, performanceMode, getCalibration, profileSummary, staleReason, type CalibrationStatus } from '../services/calibration';
import { contextSupportWarning } from '../services/contextSupport';
import { cacheConflict, flashAttentionRequired, FLASH_ATTENTION_CONFLICT_NOTE, FLASH_ATTENTION_REQUIRED_NOTE, quantizedCacheUnavailable, QUANTIZED_CACHE_UNAVAILABLE_NOTE } from '../services/cacheCompatibility';
import { Children, cloneElement, isValidElement, useEffect, useId, useRef, useState, type ReactElement, type ReactNode } from 'react';
import { getSettings, putSettings, listModels, scanModels, getRuntimePolicy, type ModelMeta } from '../services/api';
import { pushToast, type Toast } from './Toasts';
import { defaultModelOptions, type ModelListState } from './settingsModels';
import { expertSectionOpen, settingsSearchMatches, updateSetting } from './settingsForm';
import { PERMISSION_MODE_DESCRIPTIONS, PERMISSION_MODE_LABELS, PROJECT_BOUNDARY_DESCRIPTION, SEARCH_PERMISSION_DESCRIPTION } from './permissionCopy';
import { Button, Lamp } from '../ui/primitives';
import { Icon, type IconName } from '../ui/Icon';

type SetPreference = (path: string[], value: unknown) => void;
type RuntimePolicy = Awaited<ReturnType<typeof getRuntimePolicy>>;

const SECTION_ICONS: Record<string, IconName> = {
  Personalization: 'sun',
  Assistant: 'sparkle',
  'Web search': 'globe',
  Performance: 'gauge',
  'Privacy & boundaries': 'shield',
  'Expert tuning': 'sliders',
};

function Num({ obj, k, set, id }: { obj: any; k: string; set: (value: number) => void; id?: string }) {
  return <input type="number" id={id} aria-label={id ? undefined : k.replace(/_/g, ' ')}
    step={['temperature', 'top_p', 'repeat_penalty'].includes(k) ? 'any' : 1}
    value={obj?.[k] ?? ''} onChange={(event) => set(Number(event.target.value))} />;
}

/** Labels, controls, supplementary actions and help remain one indivisible row. */
export function SettingField({ label, children, description }: { label: string; children: ReactNode; description?: ReactNode }) {
  const id = useId();
  const items = Children.toArray(children);
  const first = items[0];
  const control = isValidElement(first) && (first.type === Num || ['input', 'select', 'textarea'].includes(first.type as string));
  const checkbox = control && (first as ReactElement<{ type?: string }>).props.type === 'checkbox';
  if (control) items[0] = cloneElement(first as ReactElement<{ id?: string; 'aria-describedby'?: string }>, { id, ...(description ? { 'aria-describedby': `${id}-description` } : {}) });
  return <div className={`settings-field${checkbox ? ' settings-field--toggle' : ''}`}>
    {control ? <label className="settings-field-label" htmlFor={id}>{label}</label> : <span className="settings-field-label">{label}</span>}
    <div className="settings-field-control">{items}</div>
    {description && <div className="settings-field-description" id={`${id}-description`}>{description}</div>}
  </div>;
}

export function HardwareOverrides({ settings, set }: { settings: any; set: SetPreference }) {
  if (performanceMode(settings) !== 'manual') return null;
  return <div className="settings-hardware-overrides">
    <p className="settings-capability-note">Manual values apply after Save and the next model load. Your previous values are kept when automatic management is on.</p>
    <div className="settings-fields">
      <SettingField label="CPU threads"><Num obj={settings.hardware} k="cpu_threads" set={(value) => set(['hardware', 'cpu_threads'], value)} /></SettingField>
      <SettingField label="GPU layers" description="Use −1 to let the runtime choose how many layers to place on the GPU."><Num obj={settings.hardware} k="gpu_layers" set={(value) => set(['hardware', 'gpu_layers'], value)} /></SettingField>
      <SettingField label="Flash attention" description={flashAttentionRequired(settings, performanceMode(settings)) ? FLASH_ATTENTION_REQUIRED_NOTE : cacheConflict(settings, performanceMode(settings)) ? <span className="settings-context-warning">{FLASH_ATTENTION_CONFLICT_NOTE}</span> : undefined}><input type="checkbox" className="switch" checked={!!settings.hardware?.flash_attention} disabled={flashAttentionRequired(settings, performanceMode(settings))} onChange={(event) => set(['hardware', 'flash_attention'], event.target.checked)} /></SettingField>
      <SettingField label="KV cache on GPU"><input type="checkbox" className="switch" checked={!!settings.hardware?.kv_cache_gpu} onChange={(event) => set(['hardware', 'kv_cache_gpu'], event.target.checked)} /></SettingField>
      <SettingField label="Prompt batch size"><Num obj={settings.inference} k="batch_size" set={(value) => set(['inference', 'batch_size'], value)} /></SettingField>
      <SettingField label="Prompt threads" description="Threads for reading prompts; 0 uses the CPU threads value. Prompts come in short bursts, so this can be higher than the generation threads without keeping the machine busy."><Num obj={settings.hardware} k="threads_batch" set={(value) => set(['hardware', 'threads_batch'], value)} /></SettingField>
      <SettingField label="Wait between operations" description="Spin keeps worker threads busy-waiting for the next step (slightly faster, uses CPU while idle). Sleep lets them rest."><select value={settings.hardware?.poll === 0 ? 'sleep' : 'spin'} onChange={(event) => set(['hardware', 'poll'], event.target.value === 'sleep' ? 0 : 50)}><option value="spin">Spin (runtime default)</option><option value="sleep">Sleep</option></select></SettingField>
      <SettingField label="Priority" description="Low lets other applications take the CPU first when they need it."><select value={String(settings.hardware?.priority ?? 0)} onChange={(event) => set(['hardware', 'priority'], Number(event.target.value))}><option value="0">Normal</option><option value="-1">Low</option></select></SettingField>
    </div>
  </div>;
}

/** What the selected mode means for the default model, in measured terms. */
export function PerformanceModeNote({ settings, status }: { settings: any; status: CalibrationStatus | null }) {
  const mode = performanceMode(settings);
  const base = 'Applies at the next model load. Profiles are measured per model: calibrate a model from its details on the Models page.';
  if (mode === 'auto' || mode === 'manual') return <span>{base}</span>;
  if (!settings?.general?.default_model) return <span>{base} Choose a default model to see what this profile does for it.</span>;
  const calibration = status?.calibration;
  if (!calibration) return <span>{base} The default model is not calibrated yet, so it will load with automatic settings until it is.</span>;
  const stale = staleReason(status);
  if (stale) return <span>{base} {stale}</span>;
  const profile = calibration.profiles.find((candidate) => candidate.name === mode);
  return <span>{base}{profile ? <span className="settings-mode-measured"> For the default model: {profileSummary(profile)}.</span> : null}</span>;
}

export function RuntimeSummary({ policy, dirty }: { policy: RuntimePolicy; dirty: boolean }) {
  const configuration = (value: RuntimePolicy['next']) => <dl className="settings-runtime-values">
    <div><dt>Model architecture</dt><dd>{value.architecture && value.architecture !== 'unknown' ? value.architecture : 'Determined when a model is loaded'}</dd></div>
    <div><dt>Model weights</dt><dd>{value.weights_quantization && value.weights_quantization !== 'unknown' ? value.weights_quantization : 'Determined when a model is loaded'}</dd></div>
    <div><dt>Requested cache precision</dt><dd>{value.cache_type_k} keys / {value.cache_type_v} values</dd></div>
    <div><dt>Context window</dt><dd>{value.effective_context.toLocaleString()} tokens</dd></div>
    <div><dt>CPU threads</dt><dd>{value.threads === 0 ? 'Runtime managed' : value.threads}</dd></div>
    <div><dt>GPU layers</dt><dd>{value.gpu_layers === -1 ? 'Runtime chooses' : value.gpu_layers}</dd></div>
    <div><dt>Prompt batch</dt><dd>{value.batch_size === 0 ? 'Runtime default' : value.batch_size}</dd></div>
    <div><dt>Flash attention</dt><dd>{value.flash_attention}</dd></div>
    <div><dt>Cache placement</dt><dd>{value.kv_offload}</dd></div>
    {value.placement && value.placement !== 'unknown' && <div><dt>Planned placement</dt><dd>{value.placement === 'gpu' ? 'Whole model on the GPU' : value.placement === 'hybrid' ? 'Split between GPU and system RAM (slower)' : value.placement === 'oversubscribed' ? 'Does not fit GPU + RAM (may fail or page)' : value.placement === 'cpu' ? 'CPU only' : value.placement}</dd></div>}
    <div><dt>Speculative decoding</dt><dd>{value.speculative && value.speculative !== 'none' ? `${value.speculative} (drafts from context, lossless)` : 'off'}</dd></div>
    <div><dt>Cache chunk reuse</dt><dd>{value.cache_reuse ? `${value.cache_reuse.toLocaleString()}-token minimum` : 'off'}</dd></div>
  </dl>;
  return <details className="settings-runtime-details">
    <summary>View runtime configuration</summary>
    <p className="settings-capability-note">These are requested settings, not measurements of memory use. Model-file quantization describes the weights; it does not set cache precision.</p>
    {policy.active && <div className="settings-runtime-group"><h3>Loaded session</h3>{configuration(policy.active)}{policy.active.notes.length > 0 && <ul className="settings-runtime-notes">{policy.active.notes.map((note, index) => <li key={index}>{note}</li>)}</ul>}</div>}
    {!policy.active && <p className="settings-capability-note">No managed model session is loaded.</p>}
    <div className="settings-runtime-group"><h3>Next model load</h3>
      {dirty ? <p className="settings-capability-note">Save your changes to update the planned configuration.</p> : <>{configuration(policy.next)}{policy.next.notes.length > 0 && <ul className="settings-runtime-notes">{policy.next.notes.map((note, index) => <li key={index}>{note}</li>)}</ul>}</>}
    </div>
  </details>;
}

export default function SettingsPanel({ setToasts }: { setToasts: React.Dispatch<React.SetStateAction<Toast[]>> }) {
  const [s, setS] = useState<any>(null);
  const [savedSettings, setSavedSettings] = useState<any>(null);
  const [settingsError, setSettingsError] = useState('');
  const [query, setQuery] = useState('');
  const [dirty, setDirty] = useState(false);
  const [saving, setSaving] = useState(false);
  const [expertOpen, setExpertOpen] = useState(false);
  const [expertMatches, setExpertMatches] = useState(false);
  const [matchCount, setMatchCount] = useState(0);
  const [sections, setSections] = useState<{ id: string; name: string }[]>([]);
  const [current, setCurrent] = useState('');
  const [models, setModels] = useState<ModelMeta[]>([]);
  const [calibrationStatus, setCalibrationStatus] = useState<CalibrationStatus | null>(null);
  const defaultModelId: string = s?.general?.default_model ?? '';
  useEffect(() => {
    if (!defaultModelId) { setCalibrationStatus(null); return; }
    let current = true;
    getCalibration(defaultModelId).then((next) => { if (current) setCalibrationStatus(next); }).catch(() => { if (current) setCalibrationStatus(null); });
    return () => { current = false; };
  }, [defaultModelId]);
  const [modelListState, setModelListState] = useState<ModelListState>('loading');
  const [modelListNotice, setModelListNotice] = useState('');
  const [policy, setPolicy] = useState<RuntimePolicy | null>(null);
  const [policyLoading, setPolicyLoading] = useState(true);
  const [policyError, setPolicyError] = useState('');
  const modelRequest = useRef(0);
  const policyRequest = useRef(0);
  const editRevision = useRef(0);
  const page = useRef<HTMLDivElement>(null);
  const expert = useRef<HTMLDetailsElement>(null);

  const refreshPolicy = async (modelId?: string) => {
    const request = ++policyRequest.current;
    setPolicyLoading(true);
    setPolicyError('');
    try {
      const next = await getRuntimePolicy(modelId || undefined);
      if (request === policyRequest.current) setPolicy(next);
    } catch (error) {
      if (request === policyRequest.current) setPolicyError(error instanceof Error ? error.message : 'Runtime configuration is unavailable.');
    } finally {
      if (request === policyRequest.current) setPolicyLoading(false);
    }
  };

  const refreshModels = async (rescan = false) => {
    const request = ++modelRequest.current;
    setModelListState('loading');
    setModelListNotice('');
    try {
      const scan = rescan ? await scanModels() : null;
      const detected = await listModels();
      if (request !== modelRequest.current) return;
      setModels(detected);
      setModelListState('ready');
      setModelListNotice(scan?.warnings.join(' ') ?? '');
    } catch (error) {
      if (request !== modelRequest.current) return;
      setModelListState('error');
      setModelListNotice(error instanceof Error ? error.message : 'The model list could not be loaded.');
    }
  };

  useEffect(() => {
    let active = true;
    void refreshModels();
    getSettings().then((settings) => {
      if (!active) return;
      setS(settings);
      setSavedSettings(settings);
      void refreshPolicy(settings.general?.default_model);
    }).catch((error) => { if (active) setSettingsError(error.message); });
    return () => { active = false; modelRequest.current++; policyRequest.current++; };
  }, []);

  useEffect(() => {
    const cards = Array.from(page.current?.querySelectorAll<HTMLElement>(':scope > [data-settings-title]') ?? []);
    const index: { id: string; name: string }[] = [];
    let matches = 0;
    cards.forEach((card) => {
      const name = card.dataset.settingsTitle!;
      const id = `setting-${name.toLowerCase().replace(/[^a-z0-9]+/g, '-')}`;
      card.id = id;
      card.hidden = !settingsSearchMatches(card.textContent ?? '', query);
      if (!card.hidden) matches++;
      index.push({ id, name });
    });
    setSections(index);
    setMatchCount(matches);
    setExpertMatches(expertSectionOpen(false, expert.current?.textContent ?? '', query));
  }, [s, query, models, modelListState, modelListNotice, policy, policyError, dirty]);

  // Track which section is in view so the index shows where you are.
  useEffect(() => {
    const root = page.current?.closest('.page');
    if (!root || !sections.length) return;
    const onScroll = () => {
      const cards = sections.map((section) => document.getElementById(section.id)).filter((card): card is HTMLElement => !!card && !card.hidden);
      const top = root.getBoundingClientRect().top + 80;
      let active = cards[0]?.id ?? '';
      for (const card of cards) if (card.getBoundingClientRect().top <= top) active = card.id;
      setCurrent(active);
    };
    onScroll();
    root.addEventListener('scroll', onScroll, { passive: true });
    return () => root.removeEventListener('scroll', onScroll);
  }, [sections]);

  useEffect(() => {
    const warn = (event: BeforeUnloadEvent) => { if (dirty) { event.preventDefault(); event.returnValue = ''; } };
    window.addEventListener('beforeunload', warn);
    return () => window.removeEventListener('beforeunload', warn);
  }, [dirty]);

  if (!s) return <div className="page"><div className="page-inner"><div className="panel empty-state" role="status">{settingsError ? <><Icon name="alertCircle" size={26} /><strong>Settings unavailable</strong><p>{settingsError}</p></> : <><Lamp state="caution" pulse /><p>Loading settings…</p></>}</div></div></div>;

  const set: SetPreference = (path, value) => { editRevision.current++; setDirty(true); setS((previous: any) => updateSetting(previous, path, value)); };
  const discard = () => { editRevision.current++; setS(savedSettings); setDirty(false); };
  const conflict = cacheConflict(s, performanceMode(s));
  const save = async () => {
    if (saving || conflict) return;
    setSaving(true);
    const revision = editRevision.current;
    try {
      const saved = await putSettings(s);
      if (revision === editRevision.current) { setS(saved); setSavedSettings(saved); setDirty(false); }
      window.dispatchEvent(new CustomEvent('companion:settings', { detail: saved }));
      document.documentElement.dataset.density = saved.appearance?.density ?? 'comfortable';
      document.documentElement.classList.toggle('reduce-motion', !!saved.appearance?.reduce_motion);
      if (saved.appearance?.theme) {
        localStorage.setItem('companion.theme', saved.appearance.theme);
        window.dispatchEvent(new CustomEvent('companion:appearance', { detail: saved.appearance }));
        document.documentElement.dataset.theme = saved.appearance.theme === 'system' ? (matchMedia('(prefers-color-scheme: light)').matches ? 'light' : 'dark') : saved.appearance.theme;
      }
      void refreshPolicy(saved.general?.default_model);
      pushToast(setToasts, 'success', 'Settings saved. Runtime and sampling changes apply on the next model load.');
    } catch (error) { pushToast(setToasts, 'error', error instanceof Error ? error.message : 'Settings could not be saved.'); }
    finally { setSaving(false); }
  };

  return <div className="page">
    <div className="page-inner">
      <div className="settings-layout">
        <nav className="settings-nav" aria-label="Settings categories">
          {sections.map((section) => (
            <a key={section.id} href={`#${section.id}`} aria-current={current === section.id ? 'true' : undefined} onClick={(event) => { event.preventDefault(); setQuery(''); if (section.name === 'Expert tuning') setExpertOpen(true); requestAnimationFrame(() => document.getElementById(section.id)?.scrollIntoView({ block: 'start', behavior: 'smooth' })); }}>
              <Icon name={SECTION_ICONS[section.name] ?? 'dot'} size={15} />{section.name}
            </a>
          ))}
        </nav>
        <div className="settings-page" ref={page}>
          <div className="settings-search"><Icon name="search" size={15} /><input aria-label="Search settings" placeholder="Search settings" value={query} onChange={(event) => setQuery(event.target.value)} /></div>
          {query.trim() && matchCount === 0 && <p className="settings-empty" role="status">No settings match “{query}”. Try a different term or <button type="button" onClick={() => setQuery('')}>clear search</button>.</p>}

          <section className="settings-section" data-settings-title="Personalization">
            <h2><Icon name="sun" size={16} />Personalization</h2><p className="settings-section-intro">Choose how Companion looks and what appears in your conversations.</p>
            <div className="settings-fields">
              <SettingField label="Default model" description={<><span>Preferred selection on startup; does not automatically load or switch a running model. Choose a model, then Save changes.</span><span className="settings-model-status" role={modelListState === 'error' ? 'alert' : 'status'}>{modelListState === 'loading' ? 'Looking for local models…' : modelListState === 'error' ? `Could not refresh models. ${modelListNotice}` : modelListNotice || (models.length === 0 ? 'No models detected. Add a model in Models, then refresh.' : s.general?.default_model && !models.some((model) => model.id === s.general.default_model) ? 'Your saved default is unavailable. Refresh after adding it, or choose another model.' : '')}</span></>}>
                <select value={s.general?.default_model ?? ''} disabled={modelListState === 'loading' && models.length === 0} onChange={(event) => set(['general', 'default_model'], event.target.value)}>{defaultModelOptions(models, s.general?.default_model ?? '', modelListState).map((option) => <option key={option.value} value={option.value}>{option.label}</option>)}</select>
                <button className="btn secondary" type="button" disabled={modelListState === 'loading'} onClick={() => void refreshModels(true)} aria-label={modelListState === 'error' ? 'Retry model discovery' : 'Refresh detected models'}>{modelListState === 'loading' ? 'Refreshing…' : modelListState === 'error' ? 'Retry' : 'Refresh'}</button>
              </SettingField>
              <SettingField label="Theme"><select value={s.appearance?.theme ?? s.general?.theme ?? 'dark'} onChange={(event) => set(['appearance', 'theme'], event.target.value)}><option value="dark">Dark</option><option value="light">Light</option><option value="system">Match system</option></select></SettingField>
              <SettingField label="Density"><select value={s.appearance?.density ?? 'comfortable'} onChange={(event) => set(['appearance', 'density'], event.target.value)}><option value="comfortable">Comfortable</option><option value="compact">Compact</option></select></SettingField>
              <SettingField label="Reduce motion"><input type="checkbox" className="switch" checked={!!s.appearance?.reduce_motion} onChange={(event) => set(['appearance', 'reduce_motion'], event.target.checked)} /></SettingField>
              <SettingField label="Show generation speed"><input type="checkbox" className="switch" checked={s.diagnostics?.show_generation_speed ?? true} onChange={(event) => set(['diagnostics', 'show_generation_speed'], event.target.checked)} /></SettingField>
              <SettingField label="Show detailed response metrics"><input type="checkbox" className="switch" checked={s.diagnostics?.show_detailed_metrics ?? false} onChange={(event) => set(['diagnostics', 'show_detailed_metrics'], event.target.checked)} /></SettingField>
              <SettingField label="Command palette shortcut" description="Other shortcuts: Ctrl+1 Chat, Ctrl+2 Code, Esc close, Enter send, Shift+Enter new line."><input value={s.keyboard?.command_palette ?? 'ctrl+k'} onChange={(event) => set(['keyboard', 'command_palette'], event.target.value)} /></SettingField>
            </div>
          </section>

          <section className="settings-section" data-settings-title="Assistant">
            <h2><Icon name="sparkle" size={16} />Assistant</h2><p className="settings-section-intro">Set your preferred reasoning and file-editing behavior.</p>
            <div className="settings-fields">
              <SettingField label="Permission mode" description={`${PERMISSION_MODE_DESCRIPTIONS[(s.agent?.permission_mode ?? (s.agent?.autonomous_enabled ? 'auto' : 'ask')) as keyof typeof PERMISSION_MODE_DESCRIPTIONS] ?? PERMISSION_MODE_DESCRIPTIONS.ask} ${PROJECT_BOUNDARY_DESCRIPTION} Shift+Tab in a code session cycles it.`}><select value={s.agent?.permission_mode ?? (s.agent?.autonomous_enabled ? 'auto' : 'ask')} onChange={(event) => { set(['agent', 'permission_mode'], event.target.value); set(['agent', 'autonomous_enabled'], event.target.value === 'auto'); }}>{(Object.keys(PERMISSION_MODE_LABELS) as (keyof typeof PERMISSION_MODE_LABELS)[]).map((option) => <option key={option} value={option}>{PERMISSION_MODE_LABELS[option]}</option>)}</select></SettingField>
              <SettingField label="Reasoning on by default" description="Allows longer responses; native thinking behavior depends on the model."><input type="checkbox" className="switch" checked={!!s.reasoning?.default_on} onChange={(event) => set(['reasoning', 'default_on'], event.target.checked)} /></SettingField>
              <SettingField label="Reasoning budget"><select value={s.reasoning?.budget ?? 'automatic'} onChange={(event) => set(['reasoning', 'budget'], event.target.value)}><option value="automatic">Automatic</option><option value="low">Low</option><option value="medium">Medium</option><option value="high">High</option></select></SettingField>
              <SettingField label="Automatic compaction" description="When the context fills up, older messages and agent steps are summarized so nothing is silently dropped. An agent run pauses between steps while this happens and then resumes; a chat reply starts once it is done. Original messages stay saved."><select value={s.memory?.auto_compact === 'off' ? 'off' : 'automatic'} onChange={(event) => set(['memory', 'auto_compact'], event.target.value)}><option value="automatic">Automatic</option><option value="off">Off</option></select></SettingField>
              <SettingField label="Compact at (% of usable context)" description="50–98. Usable context is the model's window minus the room kept for its next reply."><Num obj={{ compact_at_pct: s.memory?.compact_at_pct ?? 90 }} k="compact_at_pct" set={(value) => set(['memory', 'compact_at_pct'], value)} /></SettingField>
            </div>
          </section>

          <section className="settings-section" data-settings-title="Web search">
            <h2><Icon name="globe" size={16} />Web search</h2><p className="settings-section-intro">Search requests leave this PC. Enable search for each conversation request when you need it.</p>
            <div className="settings-fields">
              <SettingField label="Search provider" description={s.search?.provider === 'custom' ? 'The saved custom provider is not implemented. Choose a supported provider to use search.' : undefined}><select value={s.search?.provider ?? 'duckduckgo'} onChange={(event) => set(['search', 'provider'], event.target.value)}><option value="duckduckgo">DuckDuckGo (no key)</option><option value="brave">Brave (API key)</option>{s.search?.provider === 'custom' && <option value="custom">Saved custom provider — unavailable</option>}</select></SettingField>
              {s.search?.provider === 'brave' && <SettingField label="Brave API key"><input type="password" autoComplete="off" value={s.search?.brave_key ?? ''} onChange={(event) => set(['search', 'brave_key'], event.target.value)} /></SettingField>}
              <SettingField label="Agent search permission" description={SEARCH_PERMISSION_DESCRIPTION}><select value={s.search?.autonomous ?? 'ask'} onChange={(event) => set(['search', 'autonomous'], event.target.value)}><option value="ask">Ask unless Auto mode is on</option><option value="allow">Allow agent searches</option><option value="deny">Do not allow agent searches</option></select></SettingField>
            </div>
          </section>

          <section className="settings-section settings-performance" data-settings-title="Performance">
            <h2><Icon name="gauge" size={16} />Performance</h2><p className="settings-section-intro">Let Companion choose compatible runtime settings when you load a model.</p>
            <div className="settings-fields">
              <SettingField label="Performance mode" description={<PerformanceModeNote settings={s} status={calibrationStatus} />}>
                <div className="settings-mode-options" role="radiogroup" aria-label="Performance mode">
                  {MODES.map((option) => (
                    <label key={option.value} className={`settings-mode-option${performanceMode(s) === option.value ? ' selected' : ''}`}>
                      <input type="radio" name="performance-mode" value={option.value} checked={performanceMode(s) === option.value} onChange={() => { set(['runtime', 'mode'], option.value); set(['runtime_auto'], option.value !== 'manual'); }} />
                      <strong>{option.label}</strong>
                      <span>{option.description}</span>
                    </label>
                  ))}
                </div>
              </SettingField>
              <SettingField label="Speculative decoding" description="Auto drafts tokens that already appear in the context and verifies them in one step. The model still chooses every word; checking several words at once can very rarely pick a different one of two almost equally likely words. Replies that repeat the context (code edits, file rewrites, tool calls) finish faster, and when nothing repeats it costs no measurable speed. Applies on the next model load."><select value={s.runtime?.speculative ?? 'auto'} onChange={(event) => set(['runtime', 'speculative'], event.target.value)}><option value="auto">Auto (draft from context)</option><option value="off">Off</option></select></SettingField>
              <SettingField label="KV cache precision" description={<><span>f16 is the compatibility default. q8_0 halves cache memory, which allows a larger context on the same GPU; it measured no slower with Flash Attention on. Applies on the next model load.</span>{quantizedCacheUnavailable(s, performanceMode(s)) && <span className="settings-context-warning" role={cacheConflict(s, performanceMode(s)) ? 'alert' : 'status'}>{cacheConflict(s, performanceMode(s)) ?? QUANTIZED_CACHE_UNAVAILABLE_NOTE}</span>}</>}><select value={s.runtime?.kv_cache ?? 'f16'} onChange={(event) => set(['runtime', 'kv_cache'], event.target.value)}><option value="f16">f16 (default)</option><option value="q8_0" disabled={quantizedCacheUnavailable(s, performanceMode(s))}>q8_0 (half the cache memory{quantizedCacheUnavailable(s, performanceMode(s)) ? '; needs Flash Attention' : ''})</option></select></SettingField>
            </div>
            <HardwareOverrides settings={s} set={set} />
            <div className="settings-runtime-summary"><strong>A fresh cache for each model load</strong><p>The runtime handles cache layout for the model architecture. Loading a model starts a fresh runtime cache; your saved conversations remain on disk.</p></div>
            {policyLoading && <p className="settings-capability-note" role="status">Checking runtime configuration…</p>}
            {policyError && <p className="settings-policy-error" role="alert">Could not read runtime configuration. <button className="btn secondary sm" type="button" onClick={() => void refreshPolicy(s.general?.default_model)}>Retry</button></p>}
            {policy && !policyLoading && !policyError && <RuntimeSummary policy={policy} dirty={dirty} />}
          </section>

          <section className="settings-section" data-settings-title="Privacy & boundaries">
            <h2><Icon name="shield" size={16} />Privacy &amp; boundaries</h2>
            <ul className="settings-boundaries"><li>{PROJECT_BOUNDARY_DESCRIPTION}</li>{(Object.keys(PERMISSION_MODE_LABELS) as (keyof typeof PERMISSION_MODE_LABELS)[]).map((option) => <li key={option}>{PERMISSION_MODE_LABELS[option]}: {PERMISSION_MODE_DESCRIPTIONS[option]}</li>)}<li>{SEARCH_PERMISSION_DESCRIPTION}</li></ul>
            <p className="settings-capability-note" style={{ marginBottom: 12 }}>These are app-level controls, not an operating-system or browser sandbox. Use Auto only for tasks and projects you trust.</p>
            <div className="settings-fields">
              <SettingField label="Keep a record of model requests" description="Stores what was sent to the model and what it returned for each reply and agent step, so a wrong or broken answer can be diagnosed. Kept on this computer only, limited to the most recent 300 requests, and included when you export a conversation. Records can contain file contents the assistant read."><input type="checkbox" className="switch" checked={s.privacy?.record_model_requests !== false} onChange={(event) => set(['privacy', 'record_model_requests'], event.target.checked)} /></SettingField>
            </div>
          </section>

          <details className="settings-section settings-expert" data-settings-title="Expert tuning" ref={expert} open={expertOpen || expertMatches} onToggle={(event) => { if (!query.trim()) setExpertOpen(event.currentTarget.open); }}>
            <summary><Icon name="chevronRight" size={15} className="chev" /><span>Expert tuning</span><span className="settings-expert-caption">Context, sampling and task limits</span></summary>
            <p className="settings-section-intro">Optional overrides for specific models and workflows. Hardware stays automatic unless you change it above.</p>
            <div className="settings-fields">
              <SettingField label="Context size" description={<><span>The window to ask for. Applies on the next model load; the setting below decides what happens when it does not fit memory.</span>{contextSupportWarning(Number(s.inference?.context_size), models, s.general?.default_model) && <span className="settings-context-warning" role="status">{contextSupportWarning(Number(s.inference?.context_size), models, s.general?.default_model)}</span>}</>}><Num obj={s.inference} k="context_size" set={(value) => set(['inference', 'context_size'], value)} /></SettingField>
              <SettingField label="When the context size does not fit" description="A model plus a context cache that big may not fit the GPU. Fit it automatically keeps the whole model on the GPU by loading a smaller window, which is faster. Use my size as written keeps the window and lets part of the model run on the CPU, which is slower. Either way the loaded window and the reason are shown in the context meter."><select value={s.runtime?.context_fit === 'requested' ? 'requested' : 'fit'} onChange={(event) => set(['runtime', 'context_fit'], event.target.value)}><option value="fit">Fit it to memory (faster)</option><option value="requested">Use my size as written (slower)</option></select></SettingField>
              <SettingField label="Temperature"><Num obj={s.inference} k="temperature" set={(value) => set(['inference', 'temperature'], value)} /></SettingField>
              <SettingField label="Top-p"><Num obj={s.inference} k="top_p" set={(value) => set(['inference', 'top_p'], value)} /></SettingField>
              <SettingField label="Top-k"><Num obj={s.inference} k="top_k" set={(value) => set(['inference', 'top_k'], value)} /></SettingField>
              <SettingField label="Repeat penalty"><Num obj={s.inference} k="repeat_penalty" set={(value) => set(['inference', 'repeat_penalty'], value)} /></SettingField>
              <SettingField label="Recent messages kept after compaction"><Num obj={s.memory} k="compaction_keep_turns" set={(value) => set(['memory', 'compaction_keep_turns'], value)} /></SettingField>
              <SettingField label="Agent iteration limit"><Num obj={s.agent} k="max_iterations" set={(value) => set(['agent', 'max_iterations'], value)} /></SettingField>
              <SettingField label="Maximum search results"><Num obj={s.search} k="max_results" set={(value) => set(['search', 'max_results'], value)} /></SettingField>
              <SettingField label="Search timeout (seconds)"><Num obj={s.search} k="timeout_secs" set={(value) => set(['search', 'timeout_secs'], value)} /></SettingField>
            </div>
            <p className="settings-capability-note" style={{ marginBottom: 12 }}>Older, inactive preferences remain in your saved configuration. They are not shown as controls because the app does not apply them.</p>
          </details>

          {(dirty || saving) && (
            <div className="save-bar" role="status">
              <Lamp state="caution" />
              <span>{conflict ? 'Cannot save: an 8-bit KV cache needs Flash Attention' : 'Unsaved changes'}</span>
              <Button variant="ghost" size="sm" disabled={saving} onClick={discard}>Discard</Button>
              <Button size="sm" loading={saving} disabled={!!conflict} title={conflict ?? undefined} onClick={() => void save()}>Save changes</Button>
            </div>
          )}
        </div>
      </div>
    </div>
  </div>;
}
