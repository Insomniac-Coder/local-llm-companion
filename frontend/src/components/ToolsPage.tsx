import type { ToolDescriptor } from '../services/api';
import PluginsCard from './PluginsCard';
import { Badge, IconButton, Section } from '../ui/primitives';

type Notify = (kind: 'info' | 'success' | 'warning' | 'error', text: string) => void;

// Labels mirror the backend's risk classes; only SAFE is known to be read-only.
function risk(value: string): { tone: 'neutral' | 'warn' | 'err'; label: string } {
  const lower = value.toLowerCase();
  if (lower.startsWith('danger')) return { tone: 'err', label: 'Dangerous' };
  if (lower.startsWith('moderate')) return { tone: 'warn', label: 'Moderate' };
  return { tone: 'neutral', label: 'Read-only' };
}

export default function ToolsPage({ registry, wsId, notify, onRefresh }: { registry: ToolDescriptor[]; wsId: string; notify: Notify; onRefresh: () => void }) {
  const counts = registry.reduce((acc, tool) => { acc[risk(tool.risk).tone] += 1; return acc; }, { neutral: 0, warn: 0, err: 0 });
  return (
    <div className="page">
      <div className="page-inner">
        <div className="panel">
          <Section
            title="Tool registry"
            icon="wrench"
            meta={registry.length ? `${counts.neutral} read-only · ${counts.warn} moderate · ${counts.err} dangerous` : undefined}
            actions={<IconButton icon="refresh" label="Refresh tools" tipSide="bottom-end" onClick={onRefresh} />}
          >
            <p className="help">Tools share one permission gate with the agent. None of them can bypass your approval policy.</p>
            {registry.length === 0 && <p className="help">No tools reported. Is the runtime running?</p>}
            <div className="tool-table" role="table" aria-label="Available tools">
              {registry.map((tool) => {
                const r = risk(tool.risk);
                return (
                  <div key={tool.name} className="tool-table-row" role="row">
                    <code role="cell">{tool.name}</code>
                    <span role="cell"><Badge tone={r.tone} title={tool.risk}>{r.label}</Badge></span>
                    <span role="cell" className="tool-desc">{tool.description}</span>
                  </div>
                );
              })}
            </div>
          </Section>
        </div>
        <PluginsCard wsId={wsId} notify={notify} />
      </div>
    </div>
  );
}
