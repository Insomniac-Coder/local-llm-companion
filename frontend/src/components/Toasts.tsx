import { Button } from '../ui/primitives';

export interface Toast {
  id: number;
  kind: 'info' | 'success' | 'warning' | 'error';
  text: string;
}

const TTL: Record<Toast['kind'], number | null> = {
  info: 4000,
  success: 3500,
  warning: 7000,
  error: null, // important failures stay until dismissed (§24)
};

export function pushToast(
  set: React.Dispatch<React.SetStateAction<Toast[]>>,
  kind: Toast['kind'],
  text: string,
) {
  const id = Date.now() + Math.random();
  set((prev) => [...prev.slice(-4), { id, kind, text }]);
  const ttl = TTL[kind];
  if (ttl != null) {
    setTimeout(() => set((prev) => prev.filter((t) => t.id !== id)), ttl);
  }
}

export default function Toasts({
  toasts,
  dismiss,
}: {
  toasts: Toast[];
  dismiss: (id: number) => void;
}) {
  return (
    <div className="toasts" aria-live="polite">
      {toasts.map((t) => (
        <div key={t.id} className={`toast ${t.kind}`}>
          {t.text}
          <Button
            variant="ghost"
            size="sm"
            onClick={() => dismiss(t.id)}
            aria-label="Dismiss notification"
            title="Dismiss"
          >
            ×
          </Button>
        </div>
      ))}
    </div>
  );
}
