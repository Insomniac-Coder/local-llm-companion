import { useEffect, useState } from 'react';
import { getTimeline, type TimelineEvent } from '../services/api';

// Stage 25 session timeline (§107): activity feed, not chat clutter.
const ICON: Record<string, string> = {
  session: '●',
  attachment: '📎',
  tool: '🔧',
  artifact: '📦',
  compaction: '🗜',
};

export default function TimelinePanel({ convId }: { convId: string | null }) {
  const [events, setEvents] = useState<TimelineEvent[]>([]);
  const [open, setOpen] = useState(false);
  useEffect(() => {
    if (!convId || !open) return;
    getTimeline(convId).then((t) => setEvents(t.events)).catch(() => setEvents([]));
  }, [convId, open]);
  if (!convId) return null;
  return (
    <div className="card" style={{ margin: '0 16px' }}>
      <button className="ctx-toggle" onClick={() => setOpen((v) => !v)} aria-expanded={open}>
        {open ? '▾' : '▸'} Timeline {events.length > 0 && open ? `(${events.length})` : ''}
      </button>
      {open && (
        <div style={{ marginTop: 6, fontSize: 13 }}>
          {events.length === 0 && <div>Nothing yet — attachments, tools and artifacts appear here.</div>}
          {events.map((e, i) => (
            <div key={i} style={{ display: 'flex', gap: 8, marginTop: 2 }}>
              <span>{ICON[e.kind] ?? '•'}</span>
              <span style={{ flex: 1 }}>{e.label}</span>
              <span style={{ color: 'var(--text-muted)', fontSize: 12 }}>{e.at.slice(11, 16)}</span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
