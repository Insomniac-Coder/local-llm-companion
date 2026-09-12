import { Children, cloneElement, isValidElement, useEffect, useId, useRef, useState, type ReactElement, type ReactNode } from 'react';
import { getSettings, putSettings, listModels, scanModels, getRuntimePolicy, type ModelMeta } from '../services/api';
import { pushToast, type Toast } from './Toasts';
import { defaultModelOptions, type ModelListState } from './settingsModels';
import { expertSectionOpen, settingsSearchMatches, updateSetting } from './settingsForm';
import { AUTO_POLICY_DESCRIPTION, PROJECT_BOUNDARY_DESCRIPTION, SEARCH_PERMISSION_DESCRIPTION } from './permissionCopy';
import './settings.css';

type SetPreference = (path: string[], value: unknown) => void;
type RuntimePolicy = Awaited<ReturnType<typeof getRuntimePolicy>>;

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
  if (settings.runtime_auto !== false) return null;
  return <div className="settings-hardware-overrides">
    <p className="settings-capability-note">Manual values apply after Save and the next model load. Your previous values are kept when automatic management is on.</p>
    <div className="settings-fields">
      <SettingField label="CPU threads"><Num obj={settings.hardware} k="cpu_threads" set={(value) => set(['hardware', 'cpu_threads'], value)} /></SettingField>
      <SettingField label="GPU layers" description="Use −1 to let the runtime choose how many layers to place on the GPU."><Num obj={settings.hardware} k="gpu_layers" set={(value) => set(['hardware', 'gpu_layers'], value)} /></SettingField>
      <SettingField label="Flash attention"><input type="checkbox" checked={!!settings.hardware?.flash_attention} onChange={(event) => set(['hardware', 'flash_attention'], event.target.checked)} /></SettingField>
      <SettingField label="KV cache on GPU"><input type="checkbox" checked={!!settings.hardware?.kv_cache_gpu} onChange={(event) => set(['hardware', 'kv_cache_gpu'], event.target.checked)} /></SettingField>
      <SettingField label="Prompt batch size"><Num obj={settings.inference} k="batch_size" set={(value) => set(['inference', 'batch_size'], value)} /></SettingField>
    </div>
  </div>;
}

export function RuntimeSummary({ policy, dirty }: { policy: RuntimePolicy; dirty: boolean }) {
  const configuration = (value: RuntimePolicy['next']) => <dl className="settings-runtime-values">
    <div><dt>Model architecture</dt><dd>{value.architecture && value.architecture !== 'unknown' ? value.architecture : 'Determined when a model is loaded'}</dd></div>
    <div><dt>Model weights</dt><dd>{value.weights_quantization && value.weights_quantization !== 'unknown' ? value.weights_quantization : 'Determined when a model is loaded'}</dd></div>
    <div><dt>Requested cache precision</dt><dd>{value.cache_type_k} keys / {value.cache_type_v} values</dd></div>
    <div><dt>Context window</dt><dd>{value.effective_context.toLocaleString()} tokens</dd></div>
    <div><dt>CPU threads</dt><dd>{value.threads === 0 ? 'Runtime managed' : value.threads}</dd></div>
    <div><dt>GPU layers</dt><dd>{value.gpu_layers === -1 ? 'Runtime chooses' : value.gpu_layers}</dd></div>
    <div><dt>Prompt batch</dt><dd>{value.batch_size}</dd></div>
    <div><dt>Flash attention</dt><dd>{value.flash_attention}</dd></div>
    <div><dt>Cache placement</dt><dd>{value.kv_offload}</dd></div>
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
  const [settingsError, setSettingsError] = useState('');
  const [query, setQuery] = useState('');
  const [dirty, setDirty] = useState(false);
  const [saving, setSaving] = useState(false);
  const [expertOpen, setExpertOpen] = useState(false);
  const [expertMatches, setExpertMatches] = useState(false);
  const [matchCount, setMatchCount] = useState(0);
  const [sections, setSections] = useState<{ id: string; name: string }[]>([]);
  const [models, setModels] = useState<ModelMeta[]>([]);
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

  useEffect(() => {
    const warn = (event: BeforeUnloadEvent) => { if (dirty) { event.preventDefault(); event.returnValue = ''; } };
    window.addEventListener('beforeunload', warn);
    return () => window.removeEventListener('beforeunload', warn);
  }, [dirty]);

  if (!s) return <div className="card" role="status">{settingsError ? `Settings unavailable. ${settingsError}` : 'Loading settings…'}</div>;

  const set: SetPreference = (path, value) => { editRevision.current++; setDirty(true); setS((previous: any) => updateSetting(previous, path, value)); };
  const save = async () => {
    if (saving) return;
    setSaving(true);
    const revision = editRevision.current;
    try {
      const saved = await putSettings(s);
      if (revision === editRevision.current) { setS(saved); setDirty(false); }
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

  return <div className="chat settings-page" ref={page}>
    <div className="page-heading"><div><h1>Make it yours.</h1><p>Your preferences up front. Runtime details handled for you.</p></div></div>
    <div className="settings-toolbar"><input aria-label="Search settings" placeholder="Search settings…" value={query} onChange={(event) => setQuery(event.target.value)} /><button onClick={() => void save()} disabled={!dirty || saving}>{saving ? 'Saving…' : dirty ? 'Save changes' : 'All changes saved'}</button></div>
    <nav className="settings-sections" aria-label="Settings categories">{sections.map((section) => <a key={section.id} href={`#${section.id}`} onClick={(event) => { event.preventDefault(); setQuery(''); if (section.name === 'Expert tuning') setExpertOpen(true); requestAnimationFrame(() => document.getElementById(section.id)?.scrollIntoView({ block: 'start' })); }}>{section.name}</a>)}</nav>
    {query.trim() && matchCount === 0 && <p className="settings-empty" role="status">No settings match “{query}”. Try a different term or <button type="button" onClick={() => setQuery('')}>clear search</button>.</p>}

    <section className="card settings-section" data-settings-title="Personalization">
      <h2>Personalization</h2><p className="settings-section-intro">Choose how Companion looks and what appears in your conversations.</p>
      <div className="settings-fields">
        <SettingField label="Default model" description={<><span>Preferred selection on startup; does not automatically load or switch a running model. Choose a model, then Save changes.</span><span className="settings-model-status" role={modelListState === 'error' ? 'alert' : 'status'}>{modelListState === 'loading' ? 'Looking for local models…' : modelListState === 'error' ? `Could not refresh models. ${modelListNotice}` : modelListNotice || (models.length === 0 ? 'No models detected. Add a model in Models, then refresh.' : s.general?.default_model && !models.some((model) => model.id === s.general.default_model) ? 'Your saved default is unavailable. Refresh after adding it, or choose another model.' : '')}</span></>}>
          <select value={s.general?.default_model ?? ''} disabled={modelListState === 'loading' && models.length === 0} onChange={(event) => set(['general', 'default_model'], event.target.value)}>{defaultModelOptions(models, s.general?.default_model ?? '', modelListState).map((option) => <option key={option.value} value={option.value}>{option.label}</option>)}</select>
          <button className="settings-model-refresh" type="button" disabled={modelListState === 'loading'} onClick={() => void refreshModels(true)} aria-label={modelListState === 'error' ? 'Retry model discovery' : 'Refresh detected models'}>{modelListState === 'loading' ? 'Refreshing…' : modelListState === 'error' ? 'Retry' : 'Refresh'}</button>
        </SettingField>
        <SettingField label="Theme"><select value={s.appearance?.theme ?? s.general?.theme ?? 'dark'} onChange={(event) => set(['appearance', 'theme'], event.target.value)}><option value="dark">Dark</option><option value="light">Light</option><option value="system">System</option></select></SettingField>
        <SettingField label="Density"><select value={s.appearance?.density ?? 'comfortable'} onChange={(event) => set(['appearance', 'density'], event.target.value)}><option value="comfortable">Comfortable</option><option value="compact">Compact</option></select></SettingField>
        <SettingField label="Reduce motion"><input type="checkbox" checked={!!s.appearance?.reduce_motion} onChange={(event) => set(['appearance', 'reduce_motion'], event.target.checked)} /></SettingField>
        <SettingField label="Show generation speed"><input type="checkbox" checked={s.diagnostics?.show_generation_speed ?? true} onChange={(event) => set(['diagnostics', 'show_generation_speed'], event.target.checked)} /></SettingField>
        <SettingField label="Show detailed response metrics"><input type="checkbox" checked={s.diagnostics?.show_detailed_metrics ?? false} onChange={(event) => set(['diagnostics', 'show_detailed_metrics'], event.target.checked)} /></SettingField>
        <SettingField label="Command palette shortcut" description="Other shortcuts: Ctrl+1 Chat, Ctrl+2 Code, Esc close, Enter send, Shift+Enter new line."><input value={s.keyboard?.command_palette ?? 'ctrl+k'} onChange={(event) => set(['keyboard', 'command_palette'], event.target.value)} /></SettingField>
      </div>
    </section>

    <section className="card settings-section" data-settings-title="Assistant">
      <h2>Assistant</h2><p className="settings-section-intro">Set your preferred reasoning and file-editing behavior.</p>
      <div className="settings-fields">
        <SettingField label="Auto mode · no approval prompts" description={`${AUTO_POLICY_DESCRIPTION} ${PROJECT_BOUNDARY_DESCRIPTION}`}><input type="checkbox" checked={!!s.agent?.autonomous_enabled} onChange={(event) => set(['agent', 'autonomous_enabled'], event.target.checked)} /></SettingField>
        <SettingField label="Reasoning on by default" description="Allows longer responses; native thinking behavior depends on the model."><input type="checkbox" checked={!!s.reasoning?.default_on} onChange={(event) => set(['reasoning', 'default_on'], event.target.checked)} /></SettingField>
        <SettingField label="Reasoning budget"><select value={s.reasoning?.budget ?? 'automatic'} onChange={(event) => set(['reasoning', 'budget'], event.target.value)}><option value="automatic">Automatic</option><option value="low">Low</option><option value="medium">Medium</option><option value="high">High</option></select></SettingField>
      </div>
    </section>

    <section className="card settings-section" data-settings-title="Web search">
      <h2>Web search</h2><p className="settings-section-intro">Search requests leave this PC. Enable search for each conversation request when you need it.</p>
      <div className="settings-fields">
        <SettingField label="Search provider" description={s.search?.provider === 'custom' ? 'The saved custom provider is not implemented. Choose a supported provider to use search.' : undefined}><select value={s.search?.provider ?? 'duckduckgo'} onChange={(event) => set(['search', 'provider'], event.target.value)}><option value="duckduckgo">DuckDuckGo (no key)</option><option value="brave">Brave (API key)</option>{s.search?.provider === 'custom' && <option value="custom">Saved custom provider — unavailable</option>}</select></SettingField>
        {s.search?.provider === 'brave' && <SettingField label="Brave API key"><input type="password" autoComplete="off" value={s.search?.brave_key ?? ''} onChange={(event) => set(['search', 'brave_key'], event.target.value)} /></SettingField>}
        <SettingField label="Agent search permission" description={SEARCH_PERMISSION_DESCRIPTION}><select value={s.search?.autonomous ?? 'ask'} onChange={(event) => set(['search', 'autonomous'], event.target.value)}><option value="ask">Ask unless Auto mode is on</option><option value="allow">Allow agent searches</option><option value="deny">Do not allow agent searches</option></select></SettingField>
      </div>
    </section>

    <section className="card settings-section settings-performance" data-settings-title="Performance">
      <h2>Performance</h2><p className="settings-section-intro">Let Companion choose compatible runtime settings when you load a model.</p>
      <div className="settings-fields"><SettingField label="Manage hardware automatically" description="Chooses processor usage, GPU placement, prompt batching and cache defaults. Changes apply after Save and the next model load."><input type="checkbox" checked={s.runtime_auto !== false} onChange={(event) => set(['runtime_auto'], event.target.checked)} /></SettingField></div>
      <HardwareOverrides settings={s} set={set} />
      <div className="settings-runtime-summary"><strong>A fresh cache for each model load</strong><p>The runtime handles cache layout for the model architecture. Loading a model starts a fresh runtime cache; your saved conversations remain on disk.</p></div>
      {policyLoading && <p className="settings-capability-note" role="status">Checking runtime configuration…</p>}
      {policyError && <p className="settings-policy-error" role="alert">Could not read runtime configuration. <button type="button" onClick={() => void refreshPolicy(s.general?.default_model)}>Retry</button></p>}
      {policy && !policyLoading && !policyError && <RuntimeSummary policy={policy} dirty={dirty} />}
    </section>

    <section className="card settings-section" data-settings-title="Privacy & boundaries">
      <h2>Privacy &amp; boundaries</h2>
      <ul className="settings-boundaries"><li>{PROJECT_BOUNDARY_DESCRIPTION}</li><li>Ask mode requests approval for agent actions. {AUTO_POLICY_DESCRIPTION}</li><li>{SEARCH_PERMISSION_DESCRIPTION}</li></ul>
      <p className="settings-capability-note">These are app-level controls, not an operating-system or browser sandbox. Use Auto only for tasks and projects you trust.</p>
    </section>

    <details className="card settings-section settings-expert" data-settings-title="Expert tuning" ref={expert} open={expertOpen || expertMatches} onToggle={(event) => { if (!query.trim()) setExpertOpen(event.currentTarget.open); }}>
      <summary><span>Expert tuning</span><span className="settings-expert-caption">Context, sampling and task limits</span></summary>
      <p className="settings-section-intro">Optional overrides for specific models and workflows. Hardware stays automatic unless you change it above.</p>
      <div className="settings-fields">
        <SettingField label="Context size" description="Requested window; the effective limit may be reduced for the model. Applies on the next model load."><Num obj={s.inference} k="context_size" set={(value) => set(['inference', 'context_size'], value)} /></SettingField>
        <SettingField label="Temperature"><Num obj={s.inference} k="temperature" set={(value) => set(['inference', 'temperature'], value)} /></SettingField>
        <SettingField label="Top-p"><Num obj={s.inference} k="top_p" set={(value) => set(['inference', 'top_p'], value)} /></SettingField>
        <SettingField label="Top-k"><Num obj={s.inference} k="top_k" set={(value) => set(['inference', 'top_k'], value)} /></SettingField>
        <SettingField label="Repeat penalty"><Num obj={s.inference} k="repeat_penalty" set={(value) => set(['inference', 'repeat_penalty'], value)} /></SettingField>
        <SettingField label="Recent messages kept after compaction"><Num obj={s.memory} k="compaction_keep_turns" set={(value) => set(['memory', 'compaction_keep_turns'], value)} /></SettingField>
        <SettingField label="Agent iteration limit"><Num obj={s.agent} k="max_iterations" set={(value) => set(['agent', 'max_iterations'], value)} /></SettingField>
        <SettingField label="Maximum search results"><Num obj={s.search} k="max_results" set={(value) => set(['search', 'max_results'], value)} /></SettingField>
        <SettingField label="Search timeout (seconds)"><Num obj={s.search} k="timeout_secs" set={(value) => set(['search', 'timeout_secs'], value)} /></SettingField>
      </div>
      <p className="settings-capability-note">Older, inactive preferences remain in your saved configuration. They are not shown as controls because the app does not apply them.</p>
    </details>
  </div>;
}
