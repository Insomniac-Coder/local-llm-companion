import { useEffect, useRef, useState } from 'react';
import { IconButton } from '../ui/primitives';
import { Icon, type IconName } from '../ui/Icon';

export interface Toast {
  id: number;
  kind: 'info' | 'success' | 'warning' | 'error';
  text: string;
  /** Bumped when the same message repeats, restarting its timer instead of stacking a copy. */
  seq?: number;
}

const TTL: Record<Toast['kind'], number | null> = {
  info: 4200,
  success: 3600,
  warning: 7000,
  error: null, // important failures stay until dismissed
};

const ICONS: Record<Toast['kind'], IconName> = {
  info: 'info',
  success: 'checkCircle',
  warning: 'alert',
  error: 'alertCircle',
};

export function pushToast(
  set: React.Dispatch<React.SetStateAction<Toast[]>>,
  kind: Toast['kind'],
  text: string,
) {
  set((prev) => {
    const existing = prev.find((toast) => toast.kind === kind && toast.text === text);
    if (existing) return prev.map((toast) => toast === existing ? { ...toast, seq: (toast.seq ?? 0) + 1 } : toast);
    return [...prev.slice(-2), { id: Date.now() + Math.random(), kind, text, seq: 0 }];
  });
}

function ToastItem({ toast, dismiss }: { toast: Toast; dismiss: (id: number) => void }) {
  const [leaving, setLeaving] = useState(false);
  const [paused, setPaused] = useState(false);
  const remaining = useRef(TTL[toast.kind]);
  const startedAt = useRef(Date.now());

  useEffect(() => { remaining.current = TTL[toast.kind]; }, [toast.seq, toast.kind]);

  useEffect(() => {
    if (remaining.current == null || paused || leaving) return;
    startedAt.current = Date.now();
    const timer = window.setTimeout(() => setLeaving(true), remaining.current);
    return () => {
      window.clearTimeout(timer);
      if (remaining.current != null) remaining.current = Math.max(800, remaining.current - (Date.now() - startedAt.current));
    };
  }, [paused, leaving, toast.seq]);

  useEffect(() => {
    if (!leaving) return;
    const timer = window.setTimeout(() => dismiss(toast.id), 150);
    return () => window.clearTimeout(timer);
  }, [leaving, dismiss, toast.id]);

  return (
    <div
      className={`toast ${toast.kind}${leaving ? ' leaving' : ''}`}
      role={toast.kind === 'error' ? 'alert' : 'status'}
      onPointerEnter={() => setPaused(true)}
      onPointerLeave={() => setPaused(false)}
      onFocus={() => setPaused(true)}
      onBlur={() => setPaused(false)}
    >
      <span className="toast-icon"><Icon name={ICONS[toast.kind]} size={16} /></span>
      <span className="toast-text">{toast.text}</span>
      <IconButton icon="x" label="Dismiss notification" size="sm" tip={false} onClick={() => setLeaving(true)} />
    </div>
  );
}

/** One notification region. The app positions it just above the composer
 *  dock, or near the bottom of pages that have no composer. */
export default function Toasts({
  toasts,
  dismiss,
}: {
  toasts: Toast[];
  dismiss: (id: number) => void;
}) {
  return (
    <div className="toast-region" aria-live="polite">
      {toasts.map((toast) => <ToastItem key={toast.id} toast={toast} dismiss={dismiss} />)}
    </div>
  );
}
