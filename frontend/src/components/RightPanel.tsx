import { Tabs } from '../ui/primitives';
import { useEffect, useState } from 'react';
import { validatedPanelWidth } from '../services/workbench';

const CHAT_TABS = [
  { id: 'context', label: 'Context' },
  { id: 'files', label: 'Files' },
] as const;

const CODE_TABS = [
  { id: 'activity', label: 'Activity' },
  { id: 'files', label: 'Files' },
  { id: 'context', label: 'Context' },
  { id: 'tools', label: 'Tools' },
] as const;

export default function RightPanel({
  mode,
  tab,
  onTab,
  onClose,
  children,
}: {
  mode: 'chat' | 'code';
  tab: string;
  onTab: (id: string) => void;
  onClose: () => void;
  children: React.ReactNode;
}) {
  const tabs = mode === 'code' ? CODE_TABS : CHAT_TABS;
  const [width, setWidth] = useState(() => validatedPanelWidth(localStorage.getItem('companion.panelWidth')));
  useEffect(() => {
    document.documentElement.style.setProperty('--rightpanel-w', `${width}px`);
    localStorage.setItem('companion.panelWidth', String(width));
  }, [width]);
  return (
    <aside className="rightpanel" aria-label="Session panel">
      <div className="panel-resizer" role="separator" tabIndex={0} aria-label="Resize session panel" aria-orientation="vertical" aria-valuemin={320} aria-valuemax={640} aria-valuenow={width}
        onKeyDown={(event) => { if (event.key === 'ArrowLeft' || event.key === 'ArrowRight') { event.preventDefault(); setWidth((value) => Math.max(320, Math.min(640, value + (event.key === 'ArrowLeft' ? 20 : -20)))); } }}
        onPointerDown={(event) => { event.currentTarget.setPointerCapture(event.pointerId); }}
        onPointerMove={(event) => { if (event.currentTarget.hasPointerCapture(event.pointerId)) setWidth(Math.max(320, Math.min(640, window.innerWidth - event.clientX))); }}
        onPointerUp={(event) => event.currentTarget.releasePointerCapture(event.pointerId)} />
      <div className="rightpanel-head">
        <div>
          <Tabs tabs={tabs.map((item) => ({ ...item }))} active={tab} onChange={onTab} label="Session panel tabs" />
        </div>
        <button className="ctx-toggle" onClick={onClose} aria-label="Close session panel" title="Close panel">×</button>
      </div>
      <div className="rightpanel-body">{children}</div>
    </aside>
  );
}
