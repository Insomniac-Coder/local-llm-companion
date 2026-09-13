import { useEffect, useState } from 'react';
import { listPlugins, runPluginTool, type PluginInfo } from '../services/api';
import { Badge, Button, IconButton, Notice, Section } from '../ui/primitives';

// Plugins: manifests declare tools; runs use the same permission gate as
// built-ins. Undeclared tools never execute.
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
      notify('warning', 'Link a project first — this tool works inside a project.');
      return;
    }
    const needsApproval = !SAFE.has(tool);
    if (needsApproval && !confirm(`Plugin '${pluginId}' wants to run '${tool}', which needs approval. Continue?`)) return;
    const args = tool === 'list_directory' ? { path: '.' } : {};
    runPluginTool(pluginId, wsId, tool, args, needsApproval)
      .then((r: any) => notify(r.ok ? 'success' : 'warning', r.ok ? `Done: ${String(r.output ?? '').slice(0, 300)}` : `Failed: ${String(r.output ?? '').slice(0, 300)}`))
      .catch((e) => notify('error', e.message));
  };

  return (
    <div className="panel">
      <Section title="Plugins" icon="box" meta={plugins.length ? `${plugins.length}` : undefined} actions={<IconButton icon="refresh" label="Reload plugins" tipSide="bottom-end" onClick={() => void load()} />}>
        {plugins.length === 0 && <p className="help">No plugins found in the plugins folder.</p>}
        {plugins.map((p) => (
          <div key={p.id} className="plugin">
            <div className="plugin-head">
              <strong>{p.manifest?.name ?? p.id}</strong>
              <span className="readout">{p.manifest?.version ?? ''}</span>
              <Badge tone={p.enabled ? 'ok' : 'neutral'}>{p.enabled ? 'Enabled' : 'Disabled'}</Badge>
            </div>
            {p.manifest?.description && <p>{p.manifest.description}</p>}
            {p.error && <Notice tone="error">{p.error}</Notice>}
            {(p.unknown_tools ?? []).length > 0 && <Notice tone="caution">Unknown tools ignored: {p.unknown_tools!.join(', ')}</Notice>}
            <div className="list-rows">
              {(p.manifest?.tools ?? []).map((t) => (
                <div key={t.name} className="list-row">
                  <code className="grow">{t.name}</code>
                  <Badge tone={t.risk.toLowerCase().startsWith('danger') ? 'err' : t.risk.toLowerCase().startsWith('moderate') ? 'warn' : 'neutral'}>{t.risk}</Badge>
                  <Button size="sm" variant="ghost" icon="play" onClick={() => run(p.id, t.name, t.name !== 'system_info' && t.name !== 'list_processes')}>Run</Button>
                </div>
              ))}
            </div>
          </div>
        ))}
      </Section>
    </div>
  );
}
