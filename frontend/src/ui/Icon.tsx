import type { ReactNode } from 'react';

// One drawn icon family on a 24px grid. Text glyphs (↑ ☾ ⋯ ×) sit on font
// baselines and never centre inside a square button; these do.
const PATHS = {
  plus: <path d="M12 5v14M5 12h14" />,
  search: <><circle cx="11" cy="11" r="6.5" /><path d="m20 20-4.2-4.2" /></>,
  menu: <path d="M4.5 7h15M4.5 12h15M4.5 17h15" />,
  panelLeft: <><rect x="3.5" y="4.5" width="17" height="15" rx="2.5" /><path d="M9.5 4.5v15" /></>,
  panelRight: <><rect x="3.5" y="4.5" width="17" height="15" rx="2.5" /><path d="M14.5 4.5v15" /></>,
  more: <><circle cx="6" cy="12" r="1.5" fill="currentColor" stroke="none" /><circle cx="12" cy="12" r="1.5" fill="currentColor" stroke="none" /><circle cx="18" cy="12" r="1.5" fill="currentColor" stroke="none" /></>,
  x: <path d="M6.5 6.5l11 11M17.5 6.5l-11 11" />,
  check: <path d="M5 12.5l4.5 4.5L19 7.5" />,
  chevronDown: <path d="m6.5 9.5 5.5 5.5 5.5-5.5" />,
  chevronUp: <path d="m6.5 14.5 5.5-5.5 5.5 5.5" />,
  chevronRight: <path d="m9.5 6.5 5.5 5.5-5.5 5.5" />,
  chevronLeft: <path d="m14.5 6.5-5.5 5.5 5.5 5.5" />,
  chevronsUpDown: <path d="m8 9.5 4-4 4 4M8 14.5l4 4 4-4" />,
  arrowUp: <path d="M12 19V5.5M6 11.5l6-6 6 6" />,
  arrowDown: <path d="M12 5v13.5M6 12.5l6 6 6-6" />,
  arrowRight: <path d="M5 12h13.5M12.5 6l6 6-6 6" />,
  arrowUpRight: <path d="M7.5 16.5l9-9M9 7.5h7.5V15" />,
  stop: <rect x="7" y="7" width="10" height="10" rx="2" fill="currentColor" stroke="none" />,
  paperclip: <path d="M19 11.5l-6.9 6.9a4.5 4.5 0 0 1-6.4-6.4l7.4-7.4a3 3 0 0 1 4.2 4.2l-7.3 7.3a1.5 1.5 0 0 1-2.1-2.1l6.6-6.6" />,
  sparkle: <><path d="M11 3.5l1.8 4.9 4.9 1.8-4.9 1.8L11 16.9l-1.8-4.9L4.3 10.2l4.9-1.8z" /><path d="M18.5 15v5M16 17.5h5" /></>,
  globe: <><circle cx="12" cy="12" r="8.5" /><path d="M3.5 12h17M12 3.5c2.4 2.3 3.6 5.1 3.6 8.5s-1.2 6.2-3.6 8.5c-2.4-2.3-3.6-5.1-3.6-8.5S9.6 5.8 12 3.5z" /></>,
  chat: <path d="M5 5.5h14a1.5 1.5 0 0 1 1.5 1.5v9a1.5 1.5 0 0 1-1.5 1.5h-7l-4.5 3.5v-3.5H5A1.5 1.5 0 0 1 3.5 16V7A1.5 1.5 0 0 1 5 5.5z" />,
  code: <path d="m8.5 8-4 4 4 4M15.5 8l4 4-4 4M13.5 5.5l-3 13" />,
  folder: <path d="M3.5 7A1.5 1.5 0 0 1 5 5.5h4.2l2 2.2H19a1.5 1.5 0 0 1 1.5 1.5v8.3A1.5 1.5 0 0 1 19 19H5a1.5 1.5 0 0 1-1.5-1.5z" />,
  folderPlus: <><path d="M3.5 7A1.5 1.5 0 0 1 5 5.5h4.2l2 2.2H19a1.5 1.5 0 0 1 1.5 1.5v8.3A1.5 1.5 0 0 1 19 19H5a1.5 1.5 0 0 1-1.5-1.5z" /><path d="M12 10.5v5M9.5 13h5" /></>,
  branch: <><circle cx="7" cy="6" r="2" /><circle cx="7" cy="18" r="2" /><circle cx="17" cy="7.5" r="2" /><path d="M7 8v8M17 9.5c0 4-3.5 5.2-9.4 6.9" /></>,
  diff: <><rect x="5" y="3.5" width="14" height="17" rx="2" /><path d="M12 7.5v5M9.5 10h5M9.5 16h5" /></>,
  wrench: <path d="M14.7 6.3a4 4 0 0 0-5.4 5.4L4 17v3h3l5.3-5.3a4 4 0 0 0 5.4-5.4l-2.6 2.6-2.4-.6-.6-2.4z" />,
  flask: <path d="M9.5 3.5h5M10.5 3.5V9L5 18.5a1.5 1.5 0 0 0 1.3 2h11.4a1.5 1.5 0 0 0 1.3-2L13.5 9V3.5M7.4 14.5h9.2" />,
  play: <path d="M8 5.5v13l10.5-6.5z" />,
  pause: <path d="M9 5.5v13M15 5.5v13" />,
  layers: <path d="M12 3.5l8.5 4.5-8.5 4.5L3.5 8zM3.5 12l8.5 4.5 8.5-4.5M3.5 16l8.5 4.5 8.5-4.5" />,
  activity: <path d="M3.5 12h4l2.5-6.5 4 13 2.5-6.5h4" />,
  gauge: <><path d="M4.6 17.5a8.5 8.5 0 1 1 14.8 0" /><path d="m12 13.5 3.6-4.6" /><circle cx="12" cy="13.5" r="1.3" fill="currentColor" stroke="none" /></>,
  terminal: <><rect x="3.5" y="5" width="17" height="14" rx="2" /><path d="m7.5 10 2.5 2.5-2.5 2.5M12.5 15h4" /></>,
  sliders: <><path d="M4.5 6.5h9M17.5 6.5h2M4.5 12h3M11.5 12h8M4.5 17.5h9M17.5 17.5h2" /><circle cx="15.5" cy="6.5" r="2" /><circle cx="9.5" cy="12" r="2" /><circle cx="15.5" cy="17.5" r="2" /></>,
  sun: <><circle cx="12" cy="12" r="3.6" /><path d="M12 3v2M12 19v2M5.6 5.6 7 7M17 17l1.4 1.4M3 12h2M19 12h2M5.6 18.4 7 17M17 7l1.4-1.4" /></>,
  moon: <path d="M19.5 14.5A7.5 7.5 0 0 1 9.5 4.5a7.5 7.5 0 1 0 10 10z" />,
  monitor: <><rect x="3.5" y="4.5" width="17" height="11.5" rx="2" /><path d="M9 20h6M12 16v4" /></>,
  pin: <path d="M9.5 4.5h5l-.8 5 3.3 3V14h-10v-1.5l3.3-3zM12 14v5.5" />,
  pencil: <path d="M15.5 5.5l3 3L8 19H5v-3zM13.5 7.5l3 3" />,
  copy: <><rect x="8.5" y="8.5" width="11" height="11" rx="2" /><path d="M15.5 8.5v-2a2 2 0 0 0-2-2h-7a2 2 0 0 0-2 2v7a2 2 0 0 0 2 2h2" /></>,
  refresh: <path d="M19.5 12a7.5 7.5 0 1 1-2.2-5.3M19.5 4.5v4h-4" />,
  download: <path d="M12 4.5v10M7.5 10.5l4.5 4.5 4.5-4.5M5 19.5h14" />,
  share: <path d="M12 15V4.5M8 8.5l4-4 4 4M6 12.5v5A1.5 1.5 0 0 0 7.5 19h9a1.5 1.5 0 0 0 1.5-1.5v-5" />,
  trash: <path d="M5 7h14M9.5 7V5a1 1 0 0 1 1-1h3a1 1 0 0 1 1 1v2M7 7l.8 11.5A1.5 1.5 0 0 0 9.3 20h5.4a1.5 1.5 0 0 0 1.5-1.5L17 7M10.5 11v5M13.5 11v5" />,
  fork: <><circle cx="7" cy="5.5" r="2" /><circle cx="17" cy="5.5" r="2" /><circle cx="12" cy="18.5" r="2" /><path d="M7 7.5V9a3 3 0 0 0 3 3h4a3 3 0 0 0 3-3V7.5M12 12v4.5" /></>,
  shield: <path d="M12 3.5l7 2.5v5.5c0 4.3-2.9 7.6-7 9-4.1-1.4-7-4.7-7-9V6z" />,
  shieldCheck: <><path d="M12 3.5l7 2.5v5.5c0 4.3-2.9 7.6-7 9-4.1-1.4-7-4.7-7-9V6z" /><path d="m9 12 2 2 4-4" /></>,
  power: <path d="M12 4v7.5M7.2 7.2a7 7 0 1 0 9.6 0" />,
  eject: <path d="M12 5.5 5.5 13h13zM5.5 17.5h13" />,
  cpu: <><rect x="7" y="7" width="10" height="10" rx="1.5" /><path d="M9.5 3.5v3M14.5 3.5v3M9.5 17.5v3M14.5 17.5v3M3.5 9.5h3M3.5 14.5h3M17.5 9.5h3M17.5 14.5h3" /></>,
  memory: <><rect x="3.5" y="7" width="17" height="9" rx="1.5" /><path d="M7 10v3M10.5 10v3M14 10v3M17.5 10v3M6.5 16v2.5M17.5 16v2.5" /></>,
  alert: <path d="M12 4.5 20.5 19h-17zM12 10v4M12 16.8v.2" />,
  alertCircle: <><circle cx="12" cy="12" r="8.5" /><path d="M12 7.5V13M12 16.2v.3" /></>,
  info: <><circle cx="12" cy="12" r="8.5" /><path d="M12 11v5.5M12 7.8v.2" /></>,
  checkCircle: <><circle cx="12" cy="12" r="8.5" /><path d="m8.5 12.5 2.5 2.5 5-5.5" /></>,
  clock: <><circle cx="12" cy="12" r="8.5" /><path d="M12 7.5V12l3 2" /></>,
  file: <path d="M6.5 3.5h7l4 4v12a1 1 0 0 1-1 1h-10a1 1 0 0 1-1-1v-15a1 1 0 0 1 1-1zM13.5 3.5v4h4" />,
  fileText: <path d="M6.5 3.5h7l4 4v12a1 1 0 0 1-1 1h-10a1 1 0 0 1-1-1v-15a1 1 0 0 1 1-1zM13.5 3.5v4h4M9 12.5h6M9 16h6" />,
  filePlus: <path d="M6.5 3.5h7l4 4v12a1 1 0 0 1-1 1h-10a1 1 0 0 1-1-1v-15a1 1 0 0 1 1-1zM13.5 3.5v4h4M12 11v6M9 14h6" />,
  image: <><rect x="3.5" y="5" width="17" height="14" rx="2" /><circle cx="9" cy="10" r="1.6" /><path d="m20.5 16-4.5-4.5L7 19" /></>,
  box: <path d="M12 3.5l8 4.5v8l-8 4.5-8-4.5V8zM4 8l8 4.5L20 8M12 12.5v8" />,
  book: <path d="M5 5.5a2 2 0 0 1 2-2h12v14H7a2 2 0 0 0-2 2zM5 19.5a2 2 0 0 0 2 2h12v-4" />,
  list: <path d="M9 7h11M9 12h11M9 17h11M4.5 7h.5M4.5 12h.5M4.5 17h.5" />,
  history: <path d="M4.5 12a7.5 7.5 0 1 0 2.2-5.3M4.5 4.5v3h3M12 8v4.5l3 1.5" />,
  external: <path d="M14 4.5h5.5V10M19.5 4.5 11 13M17 13.5v5a1 1 0 0 1-1 1H6a1 1 0 0 1-1-1v-10a1 1 0 0 1 1-1h5" />,
  lock: <><rect x="5" y="10.5" width="14" height="9.5" rx="2" /><path d="M8 10.5V8a4 4 0 0 1 8 0v2.5" /></>,
  target: <><circle cx="12" cy="12" r="8.5" /><circle cx="12" cy="12" r="4.5" /><circle cx="12" cy="12" r="1" fill="currentColor" stroke="none" /></>,
  eye: <><path d="M2.5 12S6 5.5 12 5.5 21.5 12 21.5 12 18 18.5 12 18.5 2.5 12 2.5 12z" /><circle cx="12" cy="12" r="2.8" /></>,
  help: <><circle cx="12" cy="12" r="8.5" /><path d="M9.6 9.6a2.5 2.5 0 0 1 4.8.8c0 1.7-2.4 2.1-2.4 3.6M12 16.8v.2" /></>,
  dot: <circle cx="12" cy="12" r="3" fill="currentColor" stroke="none" />,
  zap: <path d="M13 3.5 5.5 13.5H12l-1 7 7.5-10H12z" />,
  plug: <path d="M9 3.5v5M15 3.5v5M7 8.5h10v3a5 5 0 0 1-10 0zM12 16.5v4" />,
  message: <path d="M4.5 6.5a2 2 0 0 1 2-2h11a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H10l-4 3.5v-3.5h.5a2 2 0 0 1-2-2z" />,
  hash: <path d="M9.5 4.5 8 19.5M16 4.5l-1.5 15M5 9h15M4 15h15" />,
} satisfies Record<string, ReactNode>;

export type IconName = keyof typeof PATHS;

export function Icon({ name, size = 16, strokeWidth = 1.8, className }: { name: IconName; size?: number; strokeWidth?: number; className?: string }) {
  return (
    <svg
      className={className ? `icon ${className}` : 'icon'}
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={strokeWidth}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      focusable="false"
    >
      {PATHS[name]}
    </svg>
  );
}
