import { useEffect, useId, useRef, useState, type ButtonHTMLAttributes, type KeyboardEvent as ReactKeyboardEvent, type ReactNode } from 'react';
import { Icon, type IconName } from './Icon';

// Primitives compose tokens only. Feature components use these rather than
// styling raw elements, so hover, focus and pressed states stay identical
// everywhere. No component in this folder imports CSS (tests bundle them).

/** Close on Escape. Every modal/popover gets this. */
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

type BtnVariant = 'default' | 'secondary' | 'primary' | 'ghost' | 'danger' | 'quiet';

export function Button({
  variant = 'secondary',
  size,
  icon,
  iconRight,
  loading,
  children,
  className,
  ...rest
}: ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: BtnVariant;
  size?: 'sm' | 'lg';
  icon?: IconName;
  iconRight?: IconName;
  loading?: boolean;
}) {
  const tone = variant === 'default' ? 'secondary' : variant;
  const cls = `btn ${tone}${size ? ` ${size}` : ''}${loading ? ' is-loading' : ''}${className ? ` ${className}` : ''}`;
  const iconSize = size === 'sm' ? 14 : 16;
  return (
    <button type="button" className={cls} disabled={loading || rest.disabled} aria-busy={loading || undefined} {...rest}>
      {loading ? <Working /> : icon ? <Icon name={icon} size={iconSize} /> : null}
      {children != null && <span className="btn-label">{children}</span>}
      {iconRight && <Icon name={iconRight} size={iconSize} />}
    </button>
  );
}

/** Square icon-only control. The label doubles as the tooltip. */
export function IconButton({
  icon,
  label,
  size = 'md',
  tone,
  pressed,
  tip = true,
  tipSide = 'bottom',
  className,
  ...rest
}: ButtonHTMLAttributes<HTMLButtonElement> & {
  icon: IconName;
  label: string;
  size?: 'sm' | 'md' | 'lg';
  tone?: 'live' | 'danger';
  pressed?: boolean;
  tip?: boolean;
  tipSide?: 'top' | 'bottom' | 'bottom-end' | 'left' | 'right';
}) {
  const px = size === 'sm' ? 14 : size === 'lg' ? 18 : 16;
  return (
    <button
      type="button"
      className={`icon-btn ${size}${tone ? ` ${tone}` : ''}${className ? ` ${className}` : ''}`}
      aria-label={label}
      aria-pressed={pressed}
      data-tip={tip ? label : undefined}
      data-tip-side={tip ? tipSide : undefined}
      {...rest}
    >
      <Icon name={icon} size={px} />
    </button>
  );
}

/** Three tally dots: the shared "machine is working" mark. */
export function Working({ tone = 'live' }: { tone?: 'live' | 'caution' | 'quiet' }) {
  return <span className={`working ${tone}`} aria-hidden="true"><i /><i /><i /></span>;
}

export function Toggle({
  on,
  icon,
  tone,
  children,
  className,
  ...rest
}: ButtonHTMLAttributes<HTMLButtonElement> & { on: boolean; icon?: IconName; tone?: 'caution' }) {
  return (
    <button type="button" className={`toggle${on ? ' on' : ''}${tone ? ` ${tone}` : ''}${className ? ` ${className}` : ''}`} aria-pressed={on} {...rest}>
      {icon && <Icon name={icon} size={15} />}
      <span>{children}</span>
    </button>
  );
}

export type Tone = 'neutral' | 'ok' | 'warn' | 'err' | 'info' | 'live';

export function Badge({ tone = 'neutral', children, title, dot }: { tone?: Tone; children: ReactNode; title?: string; dot?: boolean }) {
  return (
    <span className={`badge ${tone}`} title={title}>
      {dot && <i className="badge-dot" aria-hidden="true" />}
      {children}
    </span>
  );
}

/** Status lamp: off, ready (green), caution (amber), live (red). */
export function Lamp({ state, pulse }: { state: 'off' | 'ready' | 'caution' | 'live' | 'error'; pulse?: boolean }) {
  return <i className={`lamp ${state}${pulse ? ' pulse' : ''}`} aria-hidden="true" />;
}

export function Chip({ children, title }: { children: ReactNode; title?: string }) {
  return (
    <span className="chip" title={title}>
      <span>{children}</span>
    </span>
  );
}

export function Kbd({ children }: { children: ReactNode }) {
  return <kbd className="kbd">{children}</kbd>;
}

export function Divider() {
  return <hr className="divider" />;
}

/** Wrapper tooltip for controls that are not IconButtons. */
export function Tooltip({ tip, side = 'bottom', children }: { tip: string; side?: 'top' | 'bottom' | 'bottom-end' | 'left' | 'right'; children: ReactNode }) {
  return (
    <span className="tip-wrap" data-tip={tip} data-tip-side={side}>
      {children}
    </span>
  );
}

function focusItem(container: HTMLElement | null, index: number | 'first' | 'last') {
  const items = Array.from(container?.querySelectorAll<HTMLElement>('[role="menuitem"]:not(:disabled), [role="menuitemradio"]:not(:disabled), [role="option"]:not([aria-disabled="true"])') ?? []);
  if (!items.length) return;
  const target = index === 'first' ? items[0] : index === 'last' ? items[items.length - 1] : items[(index + items.length) % items.length];
  target.focus();
}

/** Click popover: closes on outside press and Escape; arrow keys move focus. */
export function Popover({
  open,
  onClose,
  children,
  label,
  side = 'bottom',
  align = 'end',
  className,
  autoFocus = true,
}: {
  open: boolean;
  onClose: () => void;
  children: ReactNode;
  label: string;
  side?: 'top' | 'bottom' | 'right';
  align?: 'start' | 'end';
  className?: string;
  autoFocus?: boolean;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const close = useRef(onClose);
  close.current = onClose;
  useEffect(() => {
    if (!open) return;
    const opener = document.activeElement as HTMLElement | null;
    const onDown = (e: PointerEvent) => {
      const target = e.target as Node;
      if (ref.current && !ref.current.contains(target) && !(opener && opener.contains(target))) close.current();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') { close.current(); opener?.focus?.(); }
    };
    document.addEventListener('pointerdown', onDown);
    document.addEventListener('keydown', onKey);
    if (autoFocus) requestAnimationFrame(() => focusItem(ref.current, 'first'));
    return () => {
      document.removeEventListener('pointerdown', onDown);
      document.removeEventListener('keydown', onKey);
    };
  }, [open, autoFocus]);
  if (!open) return null;
  const onKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    if (!['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) return;
    const items = Array.from(ref.current?.querySelectorAll<HTMLElement>('[role="menuitem"]:not(:disabled), [role="menuitemradio"]:not(:disabled)') ?? []);
    if (!items.length) return;
    event.preventDefault();
    const current = items.indexOf(document.activeElement as HTMLElement);
    if (event.key === 'Home') focusItem(ref.current, 'first');
    else if (event.key === 'End') focusItem(ref.current, 'last');
    else focusItem(ref.current, current + (event.key === 'ArrowDown' ? 1 : -1));
  };
  return (
    <div ref={ref} className={`pop side-${side} align-${align}${className ? ` ${className}` : ''}`} role="menu" aria-label={label} onKeyDown={onKeyDown}>
      {children}
    </div>
  );
}

export function PopItem({
  children,
  danger,
  icon,
  hint,
  checked,
  disabled,
  onClick,
}: {
  children: ReactNode;
  danger?: boolean;
  icon?: IconName;
  hint?: ReactNode;
  checked?: boolean;
  disabled?: boolean;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      className={`pop-item${danger ? ' danger' : ''}`}
      role={checked === undefined ? 'menuitem' : 'menuitemradio'}
      aria-checked={checked}
      disabled={disabled}
      onClick={onClick}
    >
      <span className="pop-item-icon">{checked ? <Icon name="check" size={15} /> : icon ? <Icon name={icon} size={15} /> : null}</span>
      <span className="pop-item-label">{children}</span>
      {hint != null && <span className="pop-item-hint">{hint}</span>}
    </button>
  );
}

export function PopLabel({ children }: { children: ReactNode }) {
  return <div className="pop-label">{children}</div>;
}

export function PopDivider() {
  return <div className="pop-divider" role="separator" />;
}

/** Accessible segmented tab strip. */
export function Tabs({
  tabs,
  active,
  onChange,
  label,
}: {
  tabs: { id: string; label: string; icon?: IconName }[];
  active: string;
  onChange: (id: string) => void;
  label: string;
}) {
  const [focus, setFocus] = useState(0);
  return (
    <div
      className="tabs"
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
          type="button"
          role="tab"
          aria-selected={t.id === active}
          tabIndex={t.id === active ? 0 : -1}
          onFocus={() => setFocus(i)}
          onClick={() => {
            setFocus(i);
            onChange(t.id);
          }}
        >
          {t.icon && <Icon name={t.icon} size={14} />}
          {t.label}
        </button>
      ))}
    </div>
  );
}

/** Inline notice with one anatomy for every banner: icon, message, actions, dismiss. */
export function Notice({
  tone = 'neutral',
  icon,
  title,
  children,
  actions,
  onDismiss,
  role,
  className,
}: {
  tone?: 'neutral' | 'caution' | 'error' | 'live' | 'ready';
  icon?: IconName;
  title?: ReactNode;
  children?: ReactNode;
  actions?: ReactNode;
  onDismiss?: () => void;
  role?: 'status' | 'alert';
  className?: string;
}) {
  const fallback: IconName = tone === 'error' ? 'alertCircle' : tone === 'caution' ? 'alert' : tone === 'ready' ? 'checkCircle' : 'info';
  return (
    <div className={`notice ${tone}${className ? ` ${className}` : ''}`} role={role ?? (tone === 'error' ? 'alert' : 'status')}>
      <span className="notice-icon"><Icon name={icon ?? fallback} size={16} /></span>
      <div className="notice-body">
        {title && <strong className="notice-title">{title}</strong>}
        {children && <div className="notice-text">{children}</div>}
      </div>
      {actions && <div className="notice-actions">{actions}</div>}
      {onDismiss && <IconButton icon="x" label="Dismiss" size="sm" tip={false} className="notice-dismiss" onClick={onDismiss} />}
    </div>
  );
}

/** Modal dialog: backdrop, focus containment on open, Escape and restore focus on close. */
export function Dialog({
  title,
  description,
  onClose,
  children,
  footer,
  size = 'md',
  role = 'dialog',
  dismissible = true,
  icon,
  className,
}: {
  title: ReactNode;
  description?: ReactNode;
  onClose: () => void;
  children?: ReactNode;
  footer?: ReactNode;
  size?: 'sm' | 'md' | 'lg' | 'xl';
  role?: 'dialog' | 'alertdialog';
  dismissible?: boolean;
  icon?: IconName;
  className?: string;
}) {
  const panel = useRef<HTMLDivElement>(null);
  const titleId = useId();
  const descriptionId = useId();
  const close = useRef(onClose);
  close.current = onClose;
  useEffect(() => {
    const opener = document.activeElement as HTMLElement | null;
    const first = panel.current?.querySelector<HTMLElement>('[autofocus], input:not([type="hidden"]), select, textarea, button:not(.dialog-close)');
    (first ?? panel.current)?.focus();
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape' && dismissible) { event.stopPropagation(); close.current(); }
      if (event.key !== 'Tab' || !panel.current) return;
      const focusable = Array.from(panel.current.querySelectorAll<HTMLElement>('button:not(:disabled), [href], input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [tabindex]:not([tabindex="-1"])'));
      if (!focusable.length) return;
      const firstEl = focusable[0];
      const lastEl = focusable[focusable.length - 1];
      if (event.shiftKey && document.activeElement === firstEl) { event.preventDefault(); lastEl.focus(); }
      else if (!event.shiftKey && document.activeElement === lastEl) { event.preventDefault(); firstEl.focus(); }
    };
    document.addEventListener('keydown', onKey);
    return () => {
      document.removeEventListener('keydown', onKey);
      opener?.focus?.();
    };
  }, [dismissible]);
  return (
    <div className="dialog-backdrop" role="presentation" onMouseDown={(event) => { if (dismissible && event.target === event.currentTarget) onClose(); }}>
      <div
        ref={panel}
        className={`dialog ${size}${className ? ` ${className}` : ''}`}
        role={role}
        aria-modal="true"
        aria-labelledby={titleId}
        aria-describedby={description ? descriptionId : undefined}
        tabIndex={-1}
      >
        <header className="dialog-head">
          {icon && <span className="dialog-icon"><Icon name={icon} size={18} /></span>}
          <div className="dialog-heading">
            <h2 id={titleId}>{title}</h2>
            {description && <p id={descriptionId}>{description}</p>}
          </div>
          {dismissible && <IconButton icon="x" label="Close" className="dialog-close" tip={false} onClick={onClose} />}
        </header>
        {children != null && <div className="dialog-body">{children}</div>}
        {footer && <footer className="dialog-foot">{footer}</footer>}
      </div>
    </div>
  );
}

/** Segmented meter: whole cells light up, so partial readings stay legible at small sizes. */
export function Meter({ value, max, cells = 12, label, warn = true }: { value: number | null | undefined; max: number | null | undefined; cells?: number; label?: string; warn?: boolean }) {
  const valid = typeof value === 'number' && Number.isFinite(value) && typeof max === 'number' && max > 0;
  const ratio = valid ? Math.max(0, Math.min(1, value! / max!)) : 0;
  const lit = valid ? Math.max(value! > 0 ? 1 : 0, Math.round(ratio * cells)) : 0;
  // Only capacity (memory) near its limit is a warning; a busy processor is normal work.
  const tone = !warn ? '' : ratio >= 0.9 ? 'hot' : ratio >= 0.75 ? 'warm' : '';
  return (
    <span className={`meter${tone ? ` ${tone}` : ''}${valid ? '' : ' unknown'}`} role="meter" aria-label={label} aria-valuemin={0} aria-valuemax={valid ? max! : undefined} aria-valuenow={valid ? value! : undefined}>
      {Array.from({ length: cells }, (_, index) => <i key={index} className={index < lit ? 'on' : ''} />)}
    </span>
  );
}

/** Collapsible panel section with a consistent header. */
export function Section({
  title,
  meta,
  actions,
  icon,
  collapsible,
  defaultOpen = true,
  children,
  className,
}: {
  title: ReactNode;
  meta?: ReactNode;
  actions?: ReactNode;
  icon?: IconName;
  collapsible?: boolean;
  defaultOpen?: boolean;
  children?: ReactNode;
  className?: string;
}) {
  const [open, setOpen] = useState(defaultOpen);
  const bodyId = useId();
  const expanded = !collapsible || open;
  return (
    <section className={`section${expanded ? ' open' : ''}${className ? ` ${className}` : ''}`}>
      <header className="section-head">
        {collapsible ? (
          <button type="button" className="section-toggle" aria-expanded={open} aria-controls={bodyId} onClick={() => setOpen((value) => !value)}>
            <Icon name="chevronRight" size={14} className="section-chevron" />
            {icon && <Icon name={icon} size={15} />}
            <span className="section-title">{title}</span>
            {meta != null && <span className="section-meta">{meta}</span>}
          </button>
        ) : (
          <div className="section-toggle static">
            {icon && <Icon name={icon} size={15} />}
            <span className="section-title">{title}</span>
            {meta != null && <span className="section-meta">{meta}</span>}
          </div>
        )}
        {actions && <div className="section-actions">{actions}</div>}
      </header>
      {expanded && <div className="section-body" id={bodyId}>{children}</div>}
    </section>
  );
}
