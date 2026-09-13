import { IconButton, Tabs } from '../ui/primitives';
import { useEffect, useState } from 'react';
import { validatedPanelWidth } from '../services/workbench';
import type { IconName } from '../ui/Icon';

const CHAT_TABS: { id: string; label: string; icon: IconName }[] = [
  { id: 'context', label: 'Context', icon: 'gauge' },
  { id: 'files', label: 'Files', icon: 'file' },
];

const CODE_TABS: { id: string; label: string; icon: IconName }[] = [
  { id: 'activity', label: 'Activity', icon: 'activity' },
  { id: 'files', label: 'Files', icon: 'file' },
  { id: 'context', label: 'Context', icon: 'gauge' },
  { id: 'tools', label: 'Tools', icon: 'wrench' },
];

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
    <aside className="inspector" aria-label="Session inspector">
      <div className="panel-resizer" role="separator" tabIndex={0} aria-label="Resize session panel" aria-orientation="vertical" aria-valuemin={320} aria-valuemax={640} aria-valuenow={width}
        onKeyDown={(event) => { if (event.key === 'ArrowLeft' || event.key === 'ArrowRight') { event.preventDefault(); setWidth((value) => Math.max(320, Math.min(640, value + (event.key === 'ArrowLeft' ? 20 : -20)))); } }}
        onPointerDown={(event) => { event.currentTarget.setPointerCapture(event.pointerId); }}
        onPointerMove={(event) => { if (event.currentTarget.hasPointerCapture(event.pointerId)) setWidth(Math.max(320, Math.min(640, window.innerWidth - event.clientX))); }}
        onPointerUp={(event) => event.currentTarget.releasePointerCapture(event.pointerId)} />
      <div className="insp-head">
        <Tabs tabs={tabs} active={tabs.some((item) => item.id === tab) ? tab : tabs[0].id} onChange={onTab} label="Inspector sections" />
        <IconButton icon="x" label="Close inspector" tipSide="bottom-end" onClick={onClose} />
      </div>
      <div className="insp-body">{children}</div>
    </aside>
  );
}
