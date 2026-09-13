import { useEffect, useState } from 'react';
import { checkCompatibility, prepareContext, type CompatInfo, type PrepStage } from '../services/api';
import { Button, Notice } from '../ui/primitives';

/**
 * Informational model-switch notice above the composer. The next request
 * rebuilds bounded history automatically; checking readiness is optional,
 * never a send gate.
 */
export default function PrepareBanner({
  convId,
  lastModel,
  loadedModel,
  loadedName,
  lastModelAvailable,
  onPrepared,
  onSwitchBack,
  onDismiss,
  notify,
}: {
  convId: string;
  convTitle?: string;
  lastModel: string;
  loadedModel: string;
  loadedName?: string;
  lastModelAvailable: boolean;
  onPrepared: () => void;
  onSwitchBack: () => void;
  onDismiss: () => void;
  notify: (kind: 'info' | 'success' | 'warning' | 'error', text: string) => void;
}) {
  const [compat, setCompat] = useState<CompatInfo | null>(null);
  const [stages, setStages] = useState<PrepStage[]>([]);
  const [running, setRunning] = useState(false);

  useEffect(() => {
    checkCompatibility(convId, loadedModel).then(setCompat).catch(() => setCompat(null));
  }, [convId, loadedModel]);

  const prepare = () => {
    setRunning(true);
    setStages([]);
    const ctl = new AbortController();
    prepareContext(convId, loadedModel, (s) => setStages((prev) => [...prev, s]), ctl.signal)
      .then((r) => {
        setRunning(false);
        if (r.ready) {
          notify('success', 'Session checked — context will be rebuilt on the next reply.');
          onPrepared();
        }
      })
      .catch((e: any) => {
        setRunning(false);
        if (e?.name !== 'AbortError') notify('error', e?.message ?? 'Preparation failed.');
      });
  };

  const warnings = compat?.warnings ?? [];
  const latest = stages[stages.length - 1];
  return (
    <Notice
      className="dock-notice"
      tone={warnings.length ? 'caution' : 'neutral'}
      icon="refresh"
      title={`Last used with ${lastModel || 'another model'}`}
      onDismiss={onDismiss}
      actions={<>
        <Button size="sm" variant="ghost" loading={running} onClick={prepare}>{running ? 'Checking…' : 'Check readiness'}</Button>
        <Button size="sm" disabled={running || !lastModelAvailable} onClick={onSwitchBack} title={!lastModelAvailable ? 'This model is no longer installed' : 'Load the previous model'}>
          {lastModelAvailable ? 'Switch back' : 'Previous model unavailable'}
        </Button>
      </>}
    >
      {running && latest ? `${latest.stage} — ${latest.detail}` : warnings.length ? warnings.join(' ') : `${loadedName ?? loadedModel} will rebuild this conversation’s context on your next message. Nothing is lost.`}
    </Notice>
  );
}
