import { useEffect, useState } from 'react';
import { addMemory, deleteMemory, listMemory, shareMemory, type MemoryEntry } from '../services/api';

// Stage 26 memory panel (§85): simple list grouped by scope, explicit share.
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
      notify('warning', scope === 'workspace' ? 'Link a workspace first.' : 'Open a conversation first.');
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
    <div className="card" style={{ margin: '0 16px' }}>
      <strong>Memory</strong>
      <div style={{ fontSize: 12, color: 'var(--text-secondary)' }}>
        Scoped recall — only referenced memories enter context (§82).
      </div>
      {Object.keys(groups).length === 0 && <div style={{ fontSize: 13 }}>No memories visible here yet.</div>}
      {Object.entries(groups).map(([sc, ms]) => (
        <div key={sc} style={{ marginTop: 6 }}>
          <div style={{ fontSize: 12, color: 'var(--text-secondary)' }}>🧠 {sc}</div>
          {ms.map((m) => (
            <div key={m.id} style={{ display: 'flex', gap: 6, fontSize: 13, marginTop: 2 }}>
              <span style={{ flex: 1 }}>{m.content.slice(0, 160)}</span>
              <button title="Delete memory" onClick={() => deleteMemory(m.id).then(load).catch((e) => notify('error', e.message))}>
                ×
              </button>
            </div>
          ))}
        </div>
      ))}
      <div style={{ display: 'flex', gap: 6, marginTop: 8 }}>
        <input value={draft} onChange={(e) => setDraft(e.target.value)} placeholder="Save as memory…" style={{ flex: 1 }} />
        <select value={scope} onChange={(e) => setScope(e.target.value)} title="Scope">
          <option value="conversation">Conversation</option>
          <option value="workspace">Workspace</option>
          <option value="global">Global</option>
        </select>
        <button onClick={save}>Save</button>
      </div>
      <div style={{ display: 'flex', gap: 6, marginTop: 6 }}>
        <input
          value={shareTarget}
          onChange={(e) => setShareTarget(e.target.value)}
          placeholder="Share first memory to conversation/workspace id…"
          style={{ flex: 1 }}
          title="Target conversation or workspace id"
        />
        <button
          disabled={items.length === 0 || !shareTarget.trim()}
          onClick={() =>
            shareMemory(items[0].id, shareTarget.trim(), 'conversation')
              .then(() => notify('success', 'Memory shared.'))
              .catch((e) => notify('error', e.message))
          }
        >
          Share
        </button>
      </div>
    </div>
  );
}
