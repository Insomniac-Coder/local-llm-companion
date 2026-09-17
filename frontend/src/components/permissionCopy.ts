export const AUTO_POLICY_DESCRIPTION = 'Auto carries out your requested task without approval prompts, including file edits, commands and deletions.';
export const PROJECT_BOUNDARY_DESCRIPTION = 'File tools stay within the selected project. Native commands are not operating-system sandboxed and can access other locations.';
export const PERMISSION_MODE_LABELS = { ask: 'Ask', accept_edits: 'Accept edits', plan: 'Plan', auto: 'Auto' } as const;
export const PERMISSION_MODE_DESCRIPTIONS = {
  ask: 'Reads and searches run freely; file edits, commands and deletions ask first.',
  accept_edits: 'Reads and file edits in the project run without asking; commands, deletions, Git and web search still ask.',
  plan: 'Read-only: the agent explores the project and proposes a plan, then you choose how to carry it out.',
  auto: AUTO_POLICY_DESCRIPTION,
} as const;
export const SEARCH_PERMISSION_DESCRIPTION = 'Web search still requires the request’s Search switch. Do not allow agent searches overrides Auto; otherwise Auto skips search approval prompts.';
