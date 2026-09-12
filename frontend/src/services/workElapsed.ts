/** Wall-clock activity age, independent of panel visibility and render lifetime. */
export function elapsedSeconds(startedAt: number | null | undefined, now = Date.now()): number | null {
  if (startedAt == null || !Number.isFinite(startedAt) || !Number.isFinite(now)) return null;
  return Math.max(0, Math.floor((now - startedAt) / 1000));
}

export function parseActivityStart(value: string | null | undefined): number | null {
  if (!value) return null;
  const time = Date.parse(value);
  return Number.isFinite(time) ? time : null;
}

export function elapsedLabel(seconds: number | null): string {
  return seconds == null ? '—' : seconds >= 60 ? `${Math.floor(seconds / 60)}m ${seconds % 60}s` : `${seconds}s`;
}

type Activity = { conversationId: string; startedAt: number | null };

/** Choose timestamp and kind together; another session's busy flag is irrelevant. */
export function visibleWorkActivity(
  conversationId: string | null,
  chatBusy: boolean,
  chat: Activity | null,
  agentBusy: boolean,
  agent: Activity | null,
): { kind: 'chat' | 'agent'; startedAt: number | null } | null {
  if (!conversationId) return null;
  if (agentBusy && agent?.conversationId === conversationId) return { kind: 'agent', startedAt: agent.startedAt };
  if (chatBusy && chat?.conversationId === conversationId) return { kind: 'chat', startedAt: chat.startedAt };
  return null;
}

export function currentActivitySnapshot(requestConversation: string, activeConversation: string | null, request: number, latest: number, starting: boolean): boolean {
  return requestConversation === activeConversation && request === latest && !starting;
}
