/**
 * Code sessions belong to one project for life. A session's history names
 * files in that project's folder, so running it anywhere else acts on the
 * wrong files (owner report, 2026-09-17: switching the project picker while an
 * old session was open moved the session, and its next task ran in the other
 * folder). The sidebar therefore groups sessions under their project, and the
 * project picker switches between groups instead of moving the open session.
 */

export type SessionLike = { id: string; workspace?: string };
export type ProjectLike = { id: string; name: string; path: string };

/** The group of sessions whose project is unset or no longer registered. */
export const NO_PROJECT = '';

/** The session's project while that project is still registered; otherwise none. */
export function sessionProject(session: Pick<SessionLike, 'workspace'>, projectIds: ReadonlySet<string>): string {
  const linked = session.workspace?.trim() ?? '';
  return linked && projectIds.has(linked) ? linked : NO_PROJECT;
}

/**
 * Whether a session may be pointed at another project. Only while nothing has
 * been asked in it yet, or when it has no registered project to keep (an older
 * session, or one whose project was removed): then choosing one is the only way
 * to use it.
 */
export function canChangeSessionProject(
  session: Pick<SessionLike, 'workspace'>,
  hasMessages: boolean,
  projectIds: ReadonlySet<string>,
): boolean {
  return !hasMessages || sessionProject(session, projectIds) === NO_PROJECT;
}

/** localStorage key for the session last opened in a project. */
export function lastSessionKey(project: string): string {
  return `companion.last.code.${project}`;
}

export type ProjectPick =
  /** The open session is already in the picked project. */
  | { kind: 'stay' }
  /** The open session has not started (or has no project): it moves to the picked project. */
  | { kind: 'move'; session: string }
  /** Switch to a session of the picked project. */
  | { kind: 'open'; session: string }
  /** Switch to the picked project with no session open, ready for a new task. */
  | { kind: 'new' };

/** What choosing a project does. The open session never changes project once work has started in it. */
export function projectPick({
  picked,
  open,
  sessions,
  projectIds,
  remembered,
  startNew = false,
}: {
  picked: string;
  /** The open code session, if any; `hasMessages` must be true while its history is still loading. */
  open?: (SessionLike & { hasMessages: boolean }) | null;
  /** Code sessions, newest first. */
  sessions: readonly SessionLike[];
  projectIds: ReadonlySet<string>;
  /** The session last opened in the picked project. */
  remembered?: string | null;
  /** The pick came from a new-task screen, so a session of that project is not reopened. */
  startNew?: boolean;
}): ProjectPick {
  if (open) {
    if (sessionProject(open, projectIds) === picked && picked !== NO_PROJECT) return { kind: 'stay' };
    if (picked !== NO_PROJECT && canChangeSessionProject(open, open.hasMessages, projectIds)) {
      return { kind: 'move', session: open.id };
    }
  }
  if (startNew) return { kind: 'new' };
  const inProject = sessions.filter((session) => sessionProject(session, projectIds) === picked);
  const session = inProject.find((candidate) => candidate.id === remembered) ?? inProject[0];
  return session ? { kind: 'open', session: session.id } : { kind: 'new' };
}

export type ProjectGroup<S extends SessionLike> = {
  project: string;
  name: string;
  path: string;
  sessions: S[];
  current: boolean;
};

/**
 * Sessions grouped by project: the current project first (shown even with no
 * sessions, unless `hideEmpty`), then the other projects in the order of their
 * newest session, then the sessions without a project.
 */
export function groupSessionsByProject<S extends SessionLike>(
  sessions: readonly S[],
  projects: readonly ProjectLike[],
  currentProject: string,
  { hideEmpty = false }: { hideEmpty?: boolean } = {},
): ProjectGroup<S>[] {
  const projectIds = new Set(projects.map((project) => project.id));
  const current = projectIds.has(currentProject) ? currentProject : NO_PROJECT;
  const bySession = new Map<string, S[]>();
  const order: string[] = [];
  for (const session of sessions) {
    const project = sessionProject(session, projectIds);
    if (!bySession.has(project)) {
      bySession.set(project, []);
      if (project !== NO_PROJECT) order.push(project);
    }
    bySession.get(project)!.push(session);
  }
  const group = (project: string): ProjectGroup<S> => {
    const known = projects.find((candidate) => candidate.id === project);
    return {
      project,
      name: known?.name ?? 'Without a project',
      path: known?.path ?? '',
      sessions: bySession.get(project) ?? [],
      current: project === current,
    };
  };
  const ids = [
    ...(current !== NO_PROJECT ? [current] : []),
    ...order.filter((project) => project !== current),
  ];
  const groups = ids.map(group);
  if (bySession.has(NO_PROJECT)) {
    const unlinked = group(NO_PROJECT);
    if (unlinked.current) groups.unshift(unlinked);
    else groups.push(unlinked);
  }
  return groups.filter((item) => item.sessions.length > 0 || (item.current && item.project !== NO_PROJECT && !hideEmpty));
}

/** Whether any session in a group needs attention: waiting for approval first, then working. */
export function groupActivity(
  sessions: readonly SessionLike[],
  activity: ReadonlyMap<string, string | undefined>,
): 'waiting' | 'working' | null {
  let working = false;
  for (const session of sessions) {
    const state = activity.get(session.id);
    if (state === 'waiting') return 'waiting';
    if (state === 'thinking' || state === 'tool') working = true;
  }
  return working ? 'working' : null;
}

/** Whether a project's group is expanded: as the user left it, otherwise only the current project. */
export function projectGroupOpen(group: { project: string; current: boolean }, saved: Readonly<Record<string, boolean>>, filtering: boolean): boolean {
  if (filtering) return true;
  const key = group.project || 'none';
  return saved[key] ?? group.current;
}

/** What removing a project would take with it, and whether it can be done now. */
export function projectRemoval({
  project,
  sessions,
  busy = false,
  activity,
}: {
  project: string;
  sessions: readonly SessionLike[];
  busy?: boolean;
  activity?: ReadonlyMap<string, string | undefined>;
}): { tasks: number; blocked: boolean; reason: string } {
  const mine = sessions.filter((session) => (session.workspace ?? '') === project);
  const working = activity
    ? mine.some((session) => ['thinking', 'tool', 'waiting'].includes(activity.get(session.id) ?? ''))
    : false;
  const blocked = busy || working || !project;
  return {
    tasks: mine.length,
    blocked,
    reason: !project
      ? 'These tasks have no project to remove.'
      : working
        ? 'A task in this project is still running. Stop it first.'
        : busy
          ? 'Wait for the current work to finish.'
          : '',
  };
}
