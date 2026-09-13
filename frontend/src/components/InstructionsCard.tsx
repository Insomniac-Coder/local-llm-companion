import { useEffect, useState } from 'react';
import { getInstructions, type InstructionFile } from '../services/api';
import { Section } from '../ui/primitives';
import { Icon } from '../ui/Icon';

// Project instructions: discovered, scoped guidance. Never a security control.
export default function InstructionsCard({ wsId }: { wsId: string }) {
  const [files, setFiles] = useState<InstructionFile[]>([]);
  useEffect(() => {
    if (!wsId) return;
    getInstructions(wsId).then((r) => setFiles(r.instructions)).catch(() => setFiles([]));
  }, [wsId]);
  if (!wsId || files.length === 0) return null;
  return (
    <Section title="Project instructions" meta={`${files.length}`} icon="book" collapsible defaultOpen={false}>
      {files.map((f) => (
        <details key={f.file} className="tool-execution">
          <summary><Icon name="chevronRight" size={13} /><code>{f.file}</code><small className="readout">{(f.chars / 1024).toFixed(1)} KB</small></summary>
          <p className="help">{f.scope}</p>
          <pre className="pre-block">{f.excerpt}</pre>
        </details>
      ))}
    </Section>
  );
}
