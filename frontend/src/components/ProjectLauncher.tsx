import { useState } from 'react';
import { createWorkspace, pickProjectFolder, type Workspace } from '../services/api';

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
    <div className="modal-backdrop" role="presentation" onMouseDown={onClose}>
      <section className="project-launcher" role="dialog" aria-modal="true" aria-label="Open a project" onMouseDown={(event) => event.stopPropagation()}>
        <header>
          <div>
            <h2>Open a project</h2>
            <p>Choose an existing folder or start with a clean one.</p>
          </div>
          <button className="icon-button" onClick={onClose} aria-label="Close project picker">×</button>
        </header>

        <div className="project-actions">
          <button className="project-action primary" disabled={busy} onClick={() => void addExisting()}>
            <span aria-hidden="true">⌁</span>
            <strong>Open existing folder</strong>
            <small>Browse your computer</small>
          </button>
          <div className="project-create">
            <label htmlFor="new-project-name">Create a new project</label>
            <div>
              <input id="new-project-name" value={name} onChange={(event) => setName(event.target.value)} placeholder="Project name" />
              <button disabled={busy || !name.trim()} onClick={() => void createNew()}>Choose location</button>
            </div>
          </div>
        </div>

        {recent.length > 0 && (
          <div className="recent-projects">
            <h3>Recent projects</h3>
            {recent.slice(0, 6).map((workspace) => (
              <button key={workspace.id} onClick={() => { onChoose(workspace); onClose(); }}>
                <span className="recent-project-mark" aria-hidden="true" />
                <span><strong>{workspace.name}</strong><small>{workspace.path}</small></span>
              </button>
            ))}
          </div>
        )}

        <details className="advanced-path">
          <summary>Enter a path manually</summary>
          <div>
            <input value={manualPath} onChange={(event) => setManualPath(event.target.value)} placeholder="C:\Projects\my-app" />
            <button disabled={busy || !manualPath.trim()} onClick={() => void addExisting(manualPath.trim())}>Open</button>
          </div>
        </details>
      </section>
    </div>
  );
}
