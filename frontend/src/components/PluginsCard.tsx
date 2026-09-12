import { useEffect, useState } from 'react';
import { listPlugins, runPluginTool, type PluginInfo } from '../services/api';

// Stage 37 plugin card (§61): manifests declare tools; runs use the same
// permission gate as built-ins. Undeclared tools never execute.
const SAFE = new Set(['list_directory', 'read_file', 'search_text', 'system_info', 'list_processes']);

export default function PluginsCard({
  wsId,
  notify,
}: {
  wsId: string;
  notify: (k: 'info' | 'success' | 'warning' | 'error', t: string) => void;
}) {
  const [plugins, setPlugins] = useState<PluginInfo[]>([]);

  const load = () => listPlugins().then((r) => setPlugins(r.plugins)).catch((e) => notify('error', e.message));
  useEffect(() => { load(); }, []);

  const run = (pluginId: string, tool: string, needsWs: boolean) => {
    if (needsWs && !wsId) {
      notify('warning', 'Link a workspace first — this tool is workspace-scoped.');
      return;
    }
    const needsApproval = !SAFE.has(tool);
    if (needsApproval && !confirm(`Plugin '${pluginId}' wants to run '${tool}' (needs approval). Continue?`)) return;
    const args = tool === 'list_directory' ? { path: '.' } : {};
    runPluginTool(pluginId, wsId, tool, args, needsApproval)
      .then((r: any) => notify(r.ok ? 'success' : 'warning', r.ok ? `Done: ${String(r.output ?? '').slice(0, 300)}` : `Failed: ${String(r.output ?? '').slice(0, 300)}`))
      .catch((e) => notify('error', e.message));
  };

  return (
    <div className="card">
      <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
        <strong>Plugins</strong>
        <span style={{ flex: 1 }} />
        <button onClick={load}>Reload</button>
      </div>
      {plugins.length === 0 && <div style={{ fontSize: 13 }}>No plugins found in plugins/.</div>}
      {plugins.map((p) => (
        <div key={p.id} style={{ marginTop: 6 }}>
          <div style={{ fontSize: 13 }}>
            <strong>{p.manifest?.name ?? p.id}</strong>{' '}
            <span style={{ color: 'var(--text-secondary)' }}>{p.manifest?.version ?? ''} · {p.enabled ? 'enabled' : 'disabled'}</span>
          </div>
          {p.error && <div className="approval" style={{ marginTop: 4 }}>{p.error}</div>}
          {p.manifest?.description && <div style={{ fontSize: 12, color: 'var(--text-secondary)' }}>{p.manifest.description}</div>}
          {(p.unknown_tools ?? []).length > 0 && (
            <div className="approval" style={{ marginTop: 4 }}>Unknown tools ignored: {p.unknown_tools!.join(', ')}</div>
          )}
          {(p.manifest?.tools ?? []).map((t) => (
            <div key={t.name} style={{ display: 'flex', gap: 8, fontSize: 13, marginTop: 2, alignItems: 'center' }}>
              <span style={{ flex: 1 }}><code>{t.name}</code> <span style={{ color: 'var(--text-secondary)' }}>{t.risk}</span></span>
              <button onClick={() => run(p.id, t.name, t.name !== 'system_info' && t.name !== 'list_processes')}>Run</button>
            </div>
          ))}
        </div>
      ))}
    </div>
  );
}
