import { useState } from 'react';
import { Popover, PopItem } from '../ui/primitives';

// Design guide §4: hover/right-click session actions — rename,
// duplicate/fork, pin, export, close. Never color-alone state.
export default function SessionMenu({
  pinned,
  onRename,
  onDuplicate,
  onTogglePin,
  onExport,
  onClose,
}: {
  pinned: boolean;
  onRename: () => void;
  onDuplicate: () => void;
  onTogglePin: () => void;
  onExport: () => void;
  onClose: () => void;
}) {
  const [open, setOpen] = useState(false);
  return (
    <>
      <button
        className="ctx-toggle convmenu-btn"
        onClick={() => setOpen((v) => !v)}
        onContextMenu={(e) => {
          e.preventDefault();
          setOpen(true);
        }}
        aria-label="Session actions"
        title="Session actions (rename, duplicate, pin, export, close)"
      >
        ⋯
      </button>
      <Popover open={open} onClose={() => setOpen(false)} label="Session actions">
        <PopItem onClick={() => { setOpen(false); onRename(); }}>Rename</PopItem>
        <PopItem onClick={() => { setOpen(false); onDuplicate(); }}>Duplicate / fork</PopItem>
        <PopItem onClick={() => { setOpen(false); onTogglePin(); }}>{pinned ? 'Unpin' : 'Pin'}</PopItem>
        <PopItem onClick={() => { setOpen(false); onExport(); }}>Export</PopItem>
        <PopItem danger onClick={() => { setOpen(false); onClose(); }}>
          Close
        </PopItem>
      </Popover>
    </>
  );
}
