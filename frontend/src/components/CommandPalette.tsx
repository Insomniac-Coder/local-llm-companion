import { useEffect, useRef, useState } from 'react';

export type QuickAction = { id: string; label: string; detail: string; run: () => void };

/** Native dialog supplies focus containment and restores focus on close. */
export default function CommandPalette({ actions, onClose }: { actions: QuickAction[]; onClose: () => void }) {
  const dialog = useRef<HTMLDialogElement>(null);
  const [query, setQuery] = useState('');
  const [selected, setSelected] = useState(0);
  const results = actions.filter((action) => `${action.label} ${action.detail}`.toLowerCase().includes(query.trim().toLowerCase())).slice(0, 30);
  useEffect(() => { dialog.current?.showModal(); }, []);
  const choose = (action: QuickAction) => { onClose(); action.run(); };
  return <dialog ref={dialog} className="command-palette" onCancel={onClose} aria-label="Find a session or action" onClick={(event) => { if (event.target === dialog.current) onClose(); }}>
    <div className="palette-content">
      <header><input autoFocus aria-label="Search sessions and actions" placeholder="Find a session or action…" value={query}
        onChange={(event) => { setQuery(event.target.value); setSelected(0); }}
        onKeyDown={(event) => {
          if (event.key === 'ArrowDown') { event.preventDefault(); setSelected((index) => Math.min(index + 1, results.length - 1)); }
          if (event.key === 'ArrowUp') { event.preventDefault(); setSelected((index) => Math.max(0, index - 1)); }
          if (event.key === 'Enter' && results[selected]) { event.preventDefault(); choose(results[selected]); }
        }} aria-controls="quick-actions" aria-activedescendant={results[selected] ? `quick-${results[selected].id}` : undefined} role="combobox" aria-expanded="true" aria-autocomplete="list" />
        <button onClick={onClose} aria-label="Close command palette">Esc</button></header>
      <div id="quick-actions" role="listbox" aria-label="Matching actions">
        {results.map((action, index) => <button key={action.id} id={`quick-${action.id}`} role="option" aria-selected={index === selected} onMouseEnter={() => setSelected(index)} onClick={() => choose(action)}>
          <span>{action.label}<small>{action.detail}</small></span><span aria-hidden="true">↵</span>
        </button>)}
        {!results.length && <p>No matches. Try a session name, “models,” or “changes.”</p>}
      </div>
      <footer><span>↑ ↓ to navigate</span><span>Enter to open</span><span>Esc to close</span></footer>
    </div>
  </dialog>;
}
