import { useState } from 'react';
import { createWorkspace, pickProjectFolder, type Workspace } from '../services/api';
import { Button, Dialog } from '../ui/primitives';
import { Icon } from '../ui/Icon';

function folderName(path: string) {
  return path.split(/[\\/]/).filter(Boolean).pop() ?? 'Project';
}

export default function ProjectLauncher({
  recent,
  onChoose,
  onClose,
  notify,
}: {
  recent: Workspace[];
  onChoose: (workspace: Workspace) => void;
  onClose: () => void;
  notify: (kind: 'success' | 'error' | 'info', text: string) => void;
}) {
  const [name, setName] = useState('');
  const [manualPath, setManualPath] = useState('');
  const [busy, setBusy] = useState(false);

  async function addExisting(path?: string) {
    setBusy(true);
    try {
      const selected = path ?? await pickProjectFolder();
      if (!selected) return;
      const workspace = await createWorkspace(folderName(selected), selected);
      onChoose(workspace);
      notify('success', `Opened ${workspace.name}.`);
      onClose();
    } catch (error: any) {
      notify('error', error?.message ?? 'Could not open that project.');
    } finally {
      setBusy(false);
    }
  }

  async function createNew() {
    if (!name.trim()) {
      notify('info', 'Name the project first.');
      return;
    }
    setBusy(true);
    try {
      const parent = await pickProjectFolder();
      if (!parent) return;
      const workspace = await createWorkspace(name.trim(), parent, true);
      onChoose(workspace);
      notify('success', `Created ${workspace.name}.`);
      onClose();
    } catch (error: any) {
      notify('error', error?.message ?? 'Could not create the project.');
    } finally {
      setBusy(false);
    }
  }

  return (
    <Dialog title="Open a project" description="The agent can only read and change files inside the project you choose." icon="folder" size="lg" onClose={onClose}>
      <div className="launch-grid">
        <button type="button" className="launch-card" disabled={busy} onClick={() => void addExisting()}>
          <Icon name="folder" size={20} />
          <strong>Open an existing folder</strong>
          <small>Browse your computer for a project you already have.</small>
        </button>
        <div className="launch-card">
          <Icon name="folderPlus" size={20} />
          <strong>Create a new project</strong>
          <small>Name it, then choose where the folder should go.</small>
          <form className="inline-form" onSubmit={(event) => { event.preventDefault(); void createNew(); }}>
            <input value={name} onChange={(event) => setName(event.target.value)} placeholder="Project name" aria-label="New project name" />
            <Button type="submit" disabled={busy || !name.trim()} style={{ height: 34 }}>Choose location</Button>
          </form>
        </div>
      </div>

      {recent.length > 0 && (
        <div className="recent-list">
          <span className="eyebrow">Recent projects</span>
          {recent.slice(0, 6).map((workspace) => (
            <button type="button" key={workspace.id} onClick={() => { onChoose(workspace); onClose(); }}>
              <Icon name="folder" size={16} />
              <span><strong>{workspace.name}</strong><small>{workspace.path}</small></span>
            </button>
          ))}
        </div>
      )}

      <details className="manual-path">
        <summary><Icon name="chevronRight" size={13} /> Enter a path instead</summary>
        <form className="inline-form" onSubmit={(event) => { event.preventDefault(); if (manualPath.trim()) void addExisting(manualPath.trim()); }}>
          <input value={manualPath} onChange={(event) => setManualPath(event.target.value)} placeholder="C:\Projects\my-app" aria-label="Project folder path" />
          <Button type="submit" disabled={busy || !manualPath.trim()} style={{ height: 34 }}>Open</Button>
        </form>
      </details>
    </Dialog>
  );
}
