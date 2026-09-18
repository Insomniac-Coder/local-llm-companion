import test from 'node:test';
import assert from 'node:assert/strict';
import {
  canChangeSessionProject, groupActivity, groupSessionsByProject, lastSessionKey, NO_PROJECT, projectGroupOpen, projectPick, projectRemoval, sessionProject,
} from './projectSessions.ts';

const projects = [
  { id: 'p-shop', name: 'shop', path: 'C:/work/shop' },
  { id: 'p-blog', name: 'blog', path: 'C:/work/blog' },
  { id: 'p-empty', name: 'empty', path: 'C:/work/empty' },
];
const ids = new Set(projects.map((project) => project.id));
// Newest first, as the conversation list arrives.
const sessions = [
  { id: 's-blog-2', workspace: 'p-blog' },
  { id: 's-shop-2', workspace: 'p-shop' },
  { id: 's-old', workspace: '' },
  { id: 's-blog-1', workspace: 'p-blog' },
  { id: 's-removed', workspace: 'p-deleted' },
  { id: 's-shop-1', workspace: 'p-shop' },
];

test('a session belongs to its project while that project is registered', () => {
  assert.equal(sessionProject({ workspace: 'p-shop' }, ids), 'p-shop');
  assert.equal(sessionProject({ workspace: '' }, ids), NO_PROJECT);
  assert.equal(sessionProject({}, ids), NO_PROJECT);
  assert.equal(sessionProject({ workspace: 'p-deleted' }, ids), NO_PROJECT, 'a removed project leaves the session without one');
});

test('a session keeps its project once work has started in it', () => {
  assert.equal(canChangeSessionProject({ workspace: 'p-shop' }, true, ids), false);
  assert.equal(canChangeSessionProject({ workspace: 'p-shop' }, false, ids), true, 'nothing asked yet');
  assert.equal(canChangeSessionProject({ workspace: '' }, true, ids), true, 'an older session without a project can be given one');
  assert.equal(canChangeSessionProject({ workspace: 'p-deleted' }, true, ids), true, 'a session whose project was removed can be given one');
});

test('picking another project while a started session is open switches to that project instead of moving the session', () => {
  const open = { id: 's-shop-2', workspace: 'p-shop', hasMessages: true };
  assert.deepEqual(projectPick({ picked: 'p-blog', open, sessions, projectIds: ids }), { kind: 'open', session: 's-blog-2' }, 'the newest session there');
  assert.deepEqual(projectPick({ picked: 'p-blog', open, sessions, projectIds: ids, remembered: 's-blog-1' }), { kind: 'open', session: 's-blog-1' }, 'the one last opened there');
  assert.deepEqual(projectPick({ picked: 'p-blog', open, sessions, projectIds: ids, remembered: 's-shop-1' }), { kind: 'open', session: 's-blog-2' }, 'a remembered session from another project is ignored');
  assert.deepEqual(projectPick({ picked: 'p-empty', open, sessions, projectIds: ids }), { kind: 'new' });
  assert.deepEqual(projectPick({ picked: 'p-blog', open, sessions, projectIds: ids, startNew: true }), { kind: 'new' });
  assert.deepEqual(projectPick({ picked: 'p-shop', open, sessions, projectIds: ids }), { kind: 'stay' });
});

test('a session still loading its history counts as started', () => {
  const loading = { id: 's-shop-2', workspace: 'p-shop', hasMessages: true };
  assert.notEqual(projectPick({ picked: 'p-blog', open: loading, sessions, projectIds: ids }).kind, 'move');
});

test('an unstarted session, or one without a project, moves to the picked project', () => {
  const fresh = { id: 's-new', workspace: 'p-shop', hasMessages: false };
  assert.deepEqual(projectPick({ picked: 'p-blog', open: fresh, sessions, projectIds: ids }), { kind: 'move', session: 's-new' });
  assert.deepEqual(projectPick({ picked: 'p-blog', open: fresh, sessions, projectIds: ids, startNew: true }), { kind: 'move', session: 's-new' });
  const older = { id: 's-old', workspace: '', hasMessages: true };
  assert.deepEqual(projectPick({ picked: 'p-blog', open: older, sessions, projectIds: ids }), { kind: 'move', session: 's-old' });
  const orphaned = { id: 's-removed', workspace: 'p-deleted', hasMessages: true };
  assert.deepEqual(projectPick({ picked: 'p-shop', open: orphaned, sessions, projectIds: ids }), { kind: 'move', session: 's-removed' });
});

test('with no session open, a pick opens that project', () => {
  assert.deepEqual(projectPick({ picked: 'p-shop', open: null, sessions, projectIds: ids }), { kind: 'open', session: 's-shop-2' });
  assert.deepEqual(projectPick({ picked: 'p-shop', open: null, sessions, projectIds: ids, startNew: true }), { kind: 'new' });
});

test('sessions are grouped under their project, the current project first', () => {
  const groups = groupSessionsByProject(sessions, projects, 'p-shop');
  assert.deepEqual(groups.map((group) => [group.project, group.name, group.current, group.sessions.map((session) => session.id)]), [
    ['p-shop', 'shop', true, ['s-shop-2', 's-shop-1']],
    ['p-blog', 'blog', false, ['s-blog-2', 's-blog-1']],
    [NO_PROJECT, 'Without a project', false, ['s-old', 's-removed']],
  ]);
  assert.equal(groups[0].path, 'C:/work/shop');
});

test('the current project shows even before its first session, other empty projects do not', () => {
  const groups = groupSessionsByProject(sessions, projects, 'p-empty');
  assert.deepEqual(groups.map((group) => [group.project, group.sessions.length]), [['p-empty', 0], ['p-blog', 2], ['p-shop', 2], [NO_PROJECT, 2]]);
  assert.deepEqual(groupSessionsByProject(sessions, projects, 'p-empty', { hideEmpty: true }).map((group) => group.project), ['p-blog', 'p-shop', NO_PROJECT], 'a filter hides it');
});

test('an open session without a project puts that group first', () => {
  const groups = groupSessionsByProject(sessions, projects, 'p-deleted');
  assert.deepEqual(groups.map((group) => [group.project, group.current]), [[NO_PROJECT, true], ['p-blog', false], ['p-shop', false]]);
  assert.deepEqual(groupSessionsByProject([], projects, ''), [], 'nothing to show before a project is chosen');
});

test('a group reports waiting for approval before work in progress', () => {
  const group = [{ id: 'a' }, { id: 'b' }, { id: 'c' }];
  assert.equal(groupActivity(group, new Map([['a', 'thinking'], ['b', 'waiting']])), 'waiting');
  assert.equal(groupActivity(group, new Map([['c', 'tool']])), 'working');
  assert.equal(groupActivity(group, new Map([['a', 'idle'], ['z', 'waiting']])), null);
});

test('groups stay as the user left them; otherwise only the current one is open', () => {
  assert.equal(projectGroupOpen({ project: 'p-shop', current: true }, {}, false), true);
  assert.equal(projectGroupOpen({ project: 'p-blog', current: false }, {}, false), false);
  assert.equal(projectGroupOpen({ project: 'p-shop', current: true }, { 'p-shop': false }, false), false);
  assert.equal(projectGroupOpen({ project: 'p-blog', current: false }, { 'p-blog': true }, false), true);
  assert.equal(projectGroupOpen({ project: NO_PROJECT, current: false }, { none: true }, false), true);
  assert.equal(projectGroupOpen({ project: 'p-blog', current: false }, { 'p-blog': false }, true), true, 'a filter opens every group with a match');
  assert.equal(lastSessionKey('p-shop'), 'companion.last.code.p-shop');
});

test('removing a project says what goes with it, and refuses while its work is running', () => {
  const sessions = [
    { id: 'a', workspace: 'p1' },
    { id: 'b', workspace: 'p1' },
    { id: 'c', workspace: 'p2' },
    { id: 'd' },
  ];
  const quiet = projectRemoval({ project: 'p1', sessions });
  assert.equal(quiet.tasks, 2);
  assert.equal(quiet.blocked, false);

  // A task of that project still working blocks it; one elsewhere does not.
  const running = projectRemoval({ project: 'p1', sessions, activity: new Map([['a', 'tool']]) });
  assert.equal(running.blocked, true);
  assert.match(running.reason, /still running/);
  assert.equal(projectRemoval({ project: 'p1', sessions, activity: new Map([['c', 'tool']]) }).blocked, false);

  // The app busy elsewhere, and the group of sessions without a project.
  assert.equal(projectRemoval({ project: 'p1', sessions, busy: true }).blocked, true);
  const none = projectRemoval({ project: '', sessions });
  assert.equal(none.blocked, true);
  assert.match(none.reason, /no project/);
});
