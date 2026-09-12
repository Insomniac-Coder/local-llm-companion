import { useEffect, useRef, useState, type ButtonHTMLAttributes, type ReactNode } from 'react';

// UI design guide §17: primitives composed from tokens only. No raw colors,
// no arbitrary spacing — feature components compose these, not raw elements.

/** Close on Escape. Every modal/popover gets this (§16 keyboard operation). */
export function useEscape(onClose: () => void) {
  const ref = useRef(onClose);
  ref.current = onClose;
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') ref.current();
    };
    document.addEventListener('keydown', onKey);
    return () => document.removeEventListener('keydown', onKey);
  }, []);
}

type BtnVariant = 'default' | 'primary' | 'ghost' | 'danger';

export function Button({
  variant = 'default',
  size,
  loading,
  children,
  className,
  ...rest
}: ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: BtnVariant;
  size?: 'sm';
  loading?: boolean;
}) {
  const cls = `ui-btn${variant !== 'default' ? ` ${variant}` : ''}${size ? ` ${size}` : ''}${loading ? ' loading' : ''}${className ? ` ${className}` : ''}`;
  return (
    <button className={cls} disabled={loading || rest.disabled} {...rest}>
      {loading ? '…' : children}
    </button>
  );
}

export function Toggle({
  on,
  children,
  ...rest
}: ButtonHTMLAttributes<HTMLButtonElement> & { on: boolean }) {
  return (
    <button className={`ui-toggle${on ? ' on' : ''}`} aria-pressed={on} {...rest}>
      {children}
    </button>
  );
}

export function Badge({
  tone = 'neutral',
  children,
  title,
}: {
  tone?: 'neutral' | 'ok' | 'warn' | 'err' | 'info';
  children: ReactNode;
  title?: string;
}) {
  const cls = `ui-badge${tone !== 'neutral' ? ` ${tone}` : ''}`;
  return (
    <span className={cls} title={title}>
      {children}
    </span>
  );
}

export function Chip({ children, title }: { children: ReactNode; title?: string }) {
  return (
    <span className="ui-chip" title={title}>
      <span>{children}</span>
    </span>
  );
}

export function Divider() {
  return <hr className="ui-divider" />;
}

/** CSS-only tooltip wrapper. Every icon-only control needs a label. */
export function Tooltip({ tip, children }: { tip: string; children: ReactNode }) {
  return (
    <span className="ui-tip" data-tip={tip}>
      {children}
    </span>
  );
}

/** Click popover: closes on outside click and Escape. */
export function Popover({
  open,
  onClose,
  children,
  label,
}: {
  open: boolean;
  onClose: () => void;
  children: ReactNode;
  label: string;
}) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) onClose();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    document.addEventListener('mousedown', onDown);
    document.addEventListener('keydown', onKey);
    return () => {
      document.removeEventListener('mousedown', onDown);
      document.removeEventListener('keydown', onKey);
    };
  }, [open, onClose]);
  if (!open) return null;
  return (
    <div ref={ref} className="ui-pop" role="menu" aria-label={label}>
      {children}
    </div>
  );
}

export function PopItem({
  children,
  danger,
  onClick,
}: {
  children: ReactNode;
  danger?: boolean;
  onClick: () => void;
}) {
  return (
    <button className={`ui-pop-item${danger ? ' danger' : ''}`} role="menuitem" onClick={onClick}>
      {children}
    </button>
  );
}

/** Accessible tab strip. */
export function Tabs({
  tabs,
  active,
  onChange,
  label,
}: {
  tabs: { id: string; label: string }[];
  active: string;
  onChange: (id: string) => void;
  label: string;
}) {
  const [focus, setFocus] = useState(0);
  return (
    <div
      className="ui-tabs"
      role="tablist"
      aria-label={label}
      onKeyDown={(e) => {
        if (e.key !== 'ArrowRight' && e.key !== 'ArrowLeft') return;
        e.preventDefault();
        const dir = e.key === 'ArrowRight' ? 1 : -1;
        const next = (focus + dir + tabs.length) % tabs.length;
        setFocus(next);
        onChange(tabs[next].id);
        (e.currentTarget.querySelectorAll<HTMLButtonElement>('[role="tab"]')[next])?.focus();
      }}
    >
      {tabs.map((t, i) => (
        <button
          key={t.id}
          role="tab"
          aria-selected={t.id === active}
          tabIndex={t.id === active ? 0 : -1}
          onFocus={() => setFocus(i)}
          onClick={() => {
            setFocus(i);
            onChange(t.id);
          }}
        >
          {t.label}
        </button>
      ))}
    </div>
  );
}
