import { useEffect, useMemo, useRef, useState } from 'react';
import { Icon, type IconName } from '../ui/Icon';
import { Kbd } from '../ui/primitives';

export type QuickAction = { id: string; label: string; detail: string; run: () => void; icon?: IconName; group?: string };

/** Native dialog supplies focus containment and restores focus on close. */
export default function CommandPalette({ actions, onClose }: { actions: QuickAction[]; onClose: () => void }) {
  const dialog = useRef<HTMLDialogElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const [query, setQuery] = useState('');
  const [selected, setSelected] = useState(0);
  const results = useMemo(() => {
    const q = query.trim().toLowerCase();
    return actions.filter((action) => `${action.label} ${action.detail} ${action.group ?? ''}`.toLowerCase().includes(q)).slice(0, 40);
  }, [actions, query]);
  useEffect(() => { dialog.current?.showModal(); }, []);
  useEffect(() => {
    list.current?.querySelector<HTMLElement>(`[data-index="${selected}"]`)?.scrollIntoView({ block: 'nearest' });
  }, [selected]);
  const choose = (action: QuickAction) => { onClose(); action.run(); };

  let lastGroup = '';
  return (
    <dialog ref={dialog} className="command-palette" onCancel={onClose} aria-label="Find a session or action" onClick={(event) => { if (event.target === dialog.current) onClose(); }}>
      <div className="palette-content">
        <div className="palette-search">
          <Icon name="search" size={18} />
          <input autoFocus aria-label="Search sessions and actions" placeholder="Search sessions, pages and actions…" value={query}
            onChange={(event) => { setQuery(event.target.value); setSelected(0); }}
            onKeyDown={(event) => {
              if (event.key === 'ArrowDown') { event.preventDefault(); setSelected((index) => Math.min(index + 1, results.length - 1)); }
              if (event.key === 'ArrowUp') { event.preventDefault(); setSelected((index) => Math.max(0, index - 1)); }
              if (event.key === 'Enter' && results[selected]) { event.preventDefault(); choose(results[selected]); }
            }} aria-controls="quick-actions" aria-activedescendant={results[selected] ? `quick-${results[selected].id}` : undefined} role="combobox" aria-expanded="true" aria-autocomplete="list" />
          <Kbd>Esc</Kbd>
        </div>
        <div id="quick-actions" className="palette-list" role="listbox" aria-label="Matching actions" ref={list}>
          {results.map((action, index) => {
            const heading = action.group && action.group !== lastGroup ? action.group : null;
            if (action.group) lastGroup = action.group;
            return (
              <div key={action.id} role="presentation">
                {heading && <div className="palette-group eyebrow" role="presentation">{heading}</div>}
                <button type="button" id={`quick-${action.id}`} data-index={index} className="palette-option" role="option" aria-selected={index === selected} onMouseMove={() => setSelected(index)} onClick={() => choose(action)}>
                  <span className="palette-option-icon"><Icon name={action.icon ?? 'arrowRight'} size={15} /></span>
                  <span className="palette-option-copy"><strong>{action.label}</strong><small>{action.detail}</small></span>
                  <Kbd>Enter</Kbd>
                </button>
              </div>
            );
          })}
          {!results.length && <p className="palette-empty">No matches. Try a session name, “models” or “theme”.</p>}
        </div>
        <footer className="palette-foot"><span><Kbd>↑</Kbd><Kbd>↓</Kbd> move</span><span><Kbd>Enter</Kbd> open</span><span><Kbd>Esc</Kbd> close</span></footer>
      </div>
    </dialog>
  );
}
