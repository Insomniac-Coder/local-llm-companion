import type { ModelMeta, Workspace } from '../services/api';
import { Button, Lamp } from '../ui/primitives';
import { Icon, type IconName } from '../ui/Icon';
import { useMachineSample, type RigState } from './Rig';
import StartScripts from './StartScripts';

type Starter = { icon: IconName; label: string; description: string; text: string };

const CHAT_STARTERS: Starter[] = [
  { icon: 'sparkle', label: 'Think it through', description: 'Talk an idea through, one question at a time.', text: 'Help me think through an idea. Ask me what I am trying to achieve.' },
  { icon: 'pencil', label: 'Draft something', description: 'A clear first draft for the audience you have in mind.', text: 'Help me write a clear first draft. Ask me about the audience and purpose.' },
  { icon: 'book', label: 'Explain a concept', description: 'Learn something step by step, at your pace.', text: 'Help me understand a concept step by step. Ask me which topic.' },
];

const CODE_STARTERS: Starter[] = [
  { icon: 'eye', label: 'Understand this project', description: 'Its purpose, main parts and how to run it.', text: 'Explore the project and explain its purpose, main components, and how to run it.' },
  { icon: 'list', label: 'Plan a change', description: 'Work out the steps before any file is touched.', text: 'Help me plan a change to this project. First, ask what I want to achieve.' },
  { icon: 'search', label: 'Find a bug', description: 'Evidence and a suggested fix, without edits.', text: 'Inspect this project for a concrete bug. Show the evidence and suggest a fix without modifying files.' },
];

const fmtContext = (tokens: number | null | undefined) => !tokens ? null : tokens >= 1000 ? `${Math.round(tokens / 102.4) / 10}K context` : `${tokens} context`;
const shortGpu = (name: string | null | undefined) => name ? name.replace(/^NVIDIA\s+/i, '').replace(/^GeForce\s+/i, '') : null;

export default function Welcome({
  mode,
  machine,
  loaded,
  selected,
  contextSize,
  workspace,
  workspaces,
  branch,
  onStarter,
  onLoad,
  onChooseModel,
  onChooseProject,
  onPickProject,
}: {
  mode: 'chat' | 'code';
  machine: RigState;
  loaded?: ModelMeta;
  selected?: ModelMeta;
  contextSize: number | null;
  workspace?: Workspace;
  workspaces: Workspace[];
  branch: string;
  onStarter: (text: string) => void;
  onLoad: () => void;
  onChooseModel: () => void;
  onChooseProject: () => void;
  onPickProject: (workspace: Workspace) => void;
}) {
  const { gpuName } = useMachineSample();
  const starters = mode === 'code' ? CODE_STARTERS : CHAT_STARTERS;
  const model = loaded ?? selected;
  const spec = model ? [model.parameters && model.parameters !== 'unknown' ? model.parameters : null, model.quantization].filter(Boolean).join(' · ') : '';

  if (mode === 'code' && !workspace) {
    return (
      <div className="welcome">
        <div className="welcome-inner">
          <div className="welcome-eyebrow eyebrow"><Icon name="code" size={14} /> Code</div>
          <h1 className="welcome-title">Choose a project to work in.</h1>
          <p className="welcome-lede">The agent can only read and change files inside the project you pick. You decide whether it asks before each action.</p>
          <div className="welcome-actions">
            <Button variant="secondary" size="lg" icon="folderPlus" onClick={onChooseProject}>Open a project</Button>
          </div>
          {workspaces.length > 0 && (
            <div className="welcome-recent">
              <span className="eyebrow">Recent projects</span>
              {workspaces.slice(0, 5).map((item) => (
                <button type="button" key={item.id} onClick={() => onPickProject(item)}>
                  <Icon name="folder" size={16} />
                  <span style={{ minWidth: 0, display: 'flex', flexDirection: 'column' }}><strong>{item.name}</strong><small>{item.path}</small></span>
                </button>
              ))}
            </div>
          )}
        </div>
      </div>
    );
  }

  return (
    <div className="welcome">
      <div className="welcome-inner">
        {mode === 'code' && workspace ? (
          <>
            <div className="welcome-eyebrow eyebrow"><Icon name="folder" size={14} /> Project</div>
            <h1 className="welcome-title">{workspace.name}</h1>
            <div className="welcome-readout">
              <span className="path">{workspace.path}</span>
              {branch && <span className="readout"><Icon name="branch" size={14} />{branch}</span>}
              {workspace.build_system && !workspace.build_system.startsWith('unknown') && <span className="readout"><Icon name="wrench" size={14} />{workspace.build_system}</span>}
            </div>
            <p className="welcome-lede">Ask about the code, work through a plan, or describe a change for the agent to make.</p>
          </>
        ) : machine === 'error' ? (
          <>
            <div className="welcome-eyebrow eyebrow"><Lamp state="error" /> Runtime offline</div>
            <h1 className="welcome-title">Companion can’t reach its runtime.</h1>
            <p className="welcome-lede">Start it again with <StartScripts />, then reload this page. Your conversations are safe on disk.</p>
          </>
        ) : machine === 'caution' && !loaded ? (
          <>
            <div className="welcome-eyebrow eyebrow"><Lamp state="caution" pulse /> Loading model</div>
            <h1 className="welcome-title">{model?.name ?? 'Preparing a model'}</h1>
            <p className="welcome-lede">Moving the model into memory. Larger models can take a minute; you can start typing now.</p>
          </>
        ) : loaded ? (
          <>
            <div className="welcome-eyebrow eyebrow"><Lamp state={machine === 'live' ? 'live' : 'ready'} pulse={machine === 'live'} /> Ready on this PC</div>
            <h1 className="welcome-title">{loaded.name}</h1>
            <div className="welcome-readout">
              {spec && <span className="readout"><Icon name="layers" size={14} />{spec}</span>}
              {fmtContext(contextSize) && <span className="readout"><Icon name="gauge" size={14} />{fmtContext(contextSize)}</span>}
              {shortGpu(gpuName) && <span className="readout"><Icon name="cpu" size={14} />{shortGpu(gpuName)}</span>}
              <span className="readout"><Icon name="lock" size={14} />Nothing leaves this PC</span>
            </div>
            <p className="welcome-lede">Think something through, draft, or make sense of a file.</p>
          </>
        ) : (
          <>
            <div className="welcome-eyebrow eyebrow"><Lamp state="off" /> No model loaded</div>
            <h1 className="welcome-title">Load a model to begin.</h1>
            <p className="welcome-lede">{selected ? <>Companion runs <strong style={{ color: 'var(--text)' }}>{selected.name}</strong> on your own hardware. Loading moves it into memory; nothing is sent anywhere.</> : 'Add a GGUF model to the models folder, then scan for it on the Models page.'}</p>
            <div className="welcome-actions">
              {selected && <Button variant="primary" size="lg" icon="power" onClick={onLoad}>Load {selected.name}</Button>}
              <Button variant="ghost" size="lg" icon="layers" onClick={onChooseModel}>{selected ? 'Choose another model' : 'Open Models'}</Button>
            </div>
          </>
        )}

        <div className="starters">
          {starters.map((starter) => (
            <button type="button" key={starter.label} className="starter" onClick={() => onStarter(starter.text)}>
              <Icon name={starter.icon} size={18} />
              <strong>{starter.label}</strong>
              <span>{starter.description}</span>
            </button>
          ))}
        </div>
      </div>
    </div>
  );
}
