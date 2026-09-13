import { useState } from 'react';
import { IconButton, PopDivider, PopItem, Popover } from '../ui/primitives';

// Row actions for a session in the sidebar. Right-click opens the same menu.
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
  const act = (fn: () => void) => () => { setOpen(false); fn(); };
  return (
    <>
      <IconButton
        icon="more"
        label="Session actions"
        size="sm"
        className="sb-row-menu"
        tip={false}
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={(event) => { event.stopPropagation(); setOpen((v) => !v); }}
        onContextMenu={(e) => {
          e.preventDefault();
          setOpen(true);
        }}
      />
      <Popover open={open} onClose={() => setOpen(false)} label="Session actions">
        <PopItem icon="pencil" onClick={act(onRename)}>Rename</PopItem>
        <PopItem icon="pin" onClick={act(onTogglePin)}>{pinned ? 'Unpin' : 'Pin to top'}</PopItem>
        <PopItem icon="fork" onClick={act(onDuplicate)}>Duplicate</PopItem>
        <PopItem icon="download" onClick={act(onExport)}>Export</PopItem>
        <PopDivider />
        <PopItem icon="trash" danger onClick={act(onClose)}>Delete…</PopItem>
      </Popover>
    </>
  );
}
