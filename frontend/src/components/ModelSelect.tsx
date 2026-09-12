import { useEffect, useRef, useState } from 'react';
import { Badge } from '../ui/primitives';
import type { ModelMeta } from '../services/api';

// Design guide §6: searchable selector showing name + quantization +
// readiness state. Loading stays an explicit separate action.
export default function ModelSelect({
  models,
  modelId,
  running,
  busy,
  onSelect,
  onLoad,
  onUnload,
  onReload,
}: {
  models: ModelMeta[];
  modelId: string;
  running: boolean;
  busy: boolean;
  onSelect: (id: string) => void;
  onLoad: () => void;
  onUnload: () => void;
  onReload: () => void;
}) {
  const [open, setOpen] = useState(false);
  const [filter, setFilter] = useState('');
  const container = useRef<HTMLSpanElement>(null);
  useEffect(() => {
    if (!open) return;
    const outside = (event: PointerEvent) => { if (!container.current?.contains(event.target as Node)) setOpen(false); };
    const escape = (event: KeyboardEvent) => { if (event.key === 'Escape') setOpen(false); };
    document.addEventListener('pointerdown', outside);
    document.addEventListener('keydown', escape);
    return () => { document.removeEventListener('pointerdown', outside); document.removeEventListener('keydown', escape); };
  }, [open]);
  const current = models.find((m) => m.id === modelId);
  const ready = !!current?.loaded && running;
  const q = filter.trim().toLowerCase();
  const list = models.filter(
    (m) => !q || `${m.name} ${m.id} ${m.parameters} ${m.quantization}`.toLowerCase().includes(q),
  );
  return (
    <span className="modelselect" ref={container}>
      <button
        onClick={() => {
          setFilter('');
          setOpen((v) => !v);
        }}
        aria-haspopup="listbox"
        aria-label={`Select model: ${current?.name ?? 'none selected'}`}
        aria-expanded={open}
        title={current ? `${current.name} · ${current.parameters} ${current.quantization}${current.loaded ? ' · loaded' : ''}` : 'Select a model'}
      >
        {current ? `${current.loaded ? '●' : '○'} ${current.name}` : '— select model —'}
      </button>
      {open && (
        <span className="ui-pop" role="listbox" aria-label="Models">
          <input
            autoFocus
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            placeholder="Search models…"
            aria-label="Search models"
            onKeyDown={(e) => {
              if (e.key === 'Escape') setOpen(false);
            }}
          />
          {list.length === 0 && <div style={{ fontSize: 12, color: 'var(--text-muted)' }}>No matches.</div>}
          {list.map((m) => (
            <button
              key={m.id}
              className="ui-pop-item"
              role="option"
              aria-selected={m.id === modelId}
              onClick={() => {
                onSelect(m.id);
                setOpen(false);
              }}
              title={`${m.parameters} ${m.quantization}${m.loaded ? ' · loaded' : ''}`}
            >
              <span style={{ flex: 1 }}>
                {m.name} <span style={{ color: 'var(--text-muted)', fontSize: 12 }}>({m.parameters} {m.quantization})</span>
              </span>
              <Badge tone={m.loaded ? 'ok' : 'neutral'}>{m.loaded ? 'Ready' : 'Idle'}</Badge>
            </button>
          ))}
        </span>
      )}
      {ready ? (
        <>
          <button className="model-action" disabled={busy} onClick={onUnload} title="Stop inference and unload the model">
            Unload
          </button>
          <button className="model-reload" disabled={busy} onClick={onReload} title="Reload this model">
            Reload
          </button>
        </>
      ) : (
        <button className="model-action" disabled={!modelId || busy} onClick={onLoad} title="Load the model and start inference">
          {busy ? 'Loading…' : 'Load'}
        </button>
      )}
    </span>
  );
}
