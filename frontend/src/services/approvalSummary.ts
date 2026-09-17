/** What an action waiting for approval will actually do, in the words the
 *  person approving needs: the command and its folder, the file, the commit
 *  message or the search. The approval card showed only the tool name, so a
 *  command was approved without seeing it. */
export function approvalTarget(tool: string, args: unknown): string {
  const value = (key: string) => {
    const field = args && typeof args === 'object' ? (args as Record<string, unknown>)[key] : undefined;
    return typeof field === 'string' ? field.trim() : '';
  };
  switch (tool) {
    case 'execute_command': {
      const command = value('command');
      const cwd = value('cwd');
      return command && cwd && cwd !== '.' ? `${command}\n(in ${cwd})` : command;
    }
    case 'git_commit':
      return value('message');
    case 'web_search':
      return value('query');
    default:
      return value('path');
  }
}
