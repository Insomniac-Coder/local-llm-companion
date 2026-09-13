import { useEffect, useState } from 'react';
import { addMemory, deleteMemory, listMemory, shareMemory, type MemoryEntry } from '../services/api';
import { Button, IconButton, Section } from '../ui/primitives';

const SCOPE_LABEL: Record<string, string> = { conversation: 'This conversation', workspace: 'This project', global: 'Everywhere' };

// Memory: a list grouped by scope. Only referenced memories enter context.
export default function MemoryPanel({
  convId,
  workspaceId,
  notify,
}: {
  convId: string | null;
  workspaceId: string;
  notify: (k: 'info' | 'success' | 'warning' | 'error', t: string) => void;
}) {
  const [items, setItems] = useState<MemoryEntry[]>([]);
  const [draft, setDraft] = useState('');
  const [scope, setScope] = useState('conversation');
  const [shareTarget, setShareTarget] = useState('');

  const load = () => {
    listMemory(convId ?? '', workspaceId).then(setItems).catch(() => setItems([]));
  };
  useEffect(load, [convId, workspaceId]);

  const save = () => {
    if (!draft.trim()) return;
    const scopeId = scope === 'global' ? '' : scope === 'workspace' ? workspaceId : convId ?? '';
    if (!scopeId && scope !== 'global') {
      notify('warning', scope === 'workspace' ? 'Link a project first.' : 'Open a conversation first.');
      return;
    }
    addMemory(draft.trim(), scope, scopeId)
      .then(() => {
        setDraft('');
        load();
        notify('success', 'Memory saved.');
      })
      .catch((e) => notify('error', e.message));
  };

  const groups: Record<string, MemoryEntry[]> = {};
  for (const m of items) {
    (groups[m.scope] = groups[m.scope] ?? []).push(m);
  }
  return (
    <Section title="Memory" meta={items.length ? `${items.length}` : undefined} icon="history" collapsible defaultOpen={items.length > 0}>
      <p className="help">Short notes the assistant can recall. Only memories relevant to a request enter its context.</p>
      {Object.keys(groups).length === 0 && <p className="help">No memories here yet.</p>}
      {Object.entries(groups).map(([sc, ms]) => (
        <div key={sc}>
          <div className="eyebrow" style={{ margin: '4px 0 2px' }}>{SCOPE_LABEL[sc] ?? sc}</div>
          <div className="list-rows">
            {ms.map((m) => (
              <div key={m.id} className="list-row">
                <span className="grow" title={m.content} style={{ whiteSpace: 'normal' }}>{m.content.slice(0, 160)}</span>
                <IconButton icon="trash" label="Delete memory" size="sm" tone="danger" tipSide="left" onClick={() => deleteMemory(m.id).then(load).catch((e) => notify('error', e.message))} />
              </div>
            ))}
          </div>
        </div>
      ))}
      <div className="inline-form">
        <input value={draft} onChange={(e) => setDraft(e.target.value)} onKeyDown={(e) => { if (e.key === 'Enter') save(); }} placeholder="Remember that…" aria-label="New memory" />
        <select value={scope} onChange={(e) => setScope(e.target.value)} aria-label="Memory scope" style={{ width: 132 }}>
          <option value="conversation">Conversation</option>
          <option value="workspace">Project</option>
          <option value="global">Everywhere</option>
        </select>
        <Button size="lg" onClick={save} disabled={!draft.trim()} style={{ height: 34 }}>Save</Button>
      </div>
      {items.length > 0 && (
        <div className="inline-form">
          <input
            value={shareTarget}
            onChange={(e) => setShareTarget(e.target.value)}
            placeholder="Conversation or project ID"
            aria-label="Share the first memory to a conversation or project ID"
          />
          <Button
            icon="share"
            style={{ height: 34 }}
            disabled={!shareTarget.trim()}
            onClick={() =>
              shareMemory(items[0].id, shareTarget.trim(), 'conversation')
                .then(() => notify('success', 'Memory shared.'))
                .catch((e) => notify('error', e.message))
            }
          >
            Share
          </Button>
        </div>
      )}
    </Section>
  );
}
