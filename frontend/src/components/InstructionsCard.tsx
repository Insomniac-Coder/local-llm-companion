import { useEffect, useState } from 'react';
import { getInstructions, type InstructionFile } from '../services/api';

// Stage 24 project instructions (§90): discovered, scoped, never security.
export default function InstructionsCard({ wsId }: { wsId: string }) {
  const [files, setFiles] = useState<InstructionFile[]>([]);
  const [open, setOpen] = useState(false);
  useEffect(() => {
    if (!wsId) return;
    getInstructions(wsId).then((r) => setFiles(r.instructions)).catch(() => setFiles([]));
  }, [wsId]);
  if (!wsId || files.length === 0) return null;
  return (
    <div className="card" style={{ margin: '0 16px' }}>
      <button className="ctx-toggle" onClick={() => setOpen((v) => !v)} aria-expanded={open}>
        {open ? '▾' : '▸'} Project instructions ({files.length})
      </button>
      {open &&
        files.map((f) => (
          <details key={f.file} style={{ marginTop: 4 }}>
            <summary>
              {f.file} · {(f.chars / 1024).toFixed(1)} KB
            </summary>
            <div style={{ fontSize: 12, color: 'var(--text-secondary)' }}>{f.scope}</div>
            <pre className="diff-body">{f.excerpt}</pre>
          </details>
        ))}
    </div>
  );
}
