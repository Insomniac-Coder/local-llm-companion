// Stage 22: typed local event protocol (§§39–40).
// The UI derives state from these structured events, never by parsing
// human-readable log strings.

export type AppEventType =
  | 'message.delta'
  | 'message.complete'
  | 'operation.progress'
  | 'tool.started'
  | 'tool.output'
  | 'tool.completed'
  | 'tool.failed'
  | 'agent.state'
  | 'permission.requested'
  | 'search.started'
  | 'search.result'
  | 'search.completed'
  | 'artifact.created'
  | 'operation.cancelled'
  | 'error';

export interface AppEvent {
  type: AppEventType;
  operation_id?: string;
  operation?: string;
  progress?: number;
  message?: string;
  timestamp?: number;
  [k: string]: unknown;
}

export function isTerminalAgentState(state: string): boolean {
  return ['COMPLETED', 'FAILED', 'CANCELLED'].includes(state);
}

export function activityLabel(activity?: string): string {
  switch (activity) {
    case 'thinking':
      return '● Thinking';
    case 'tool':
      return '◌ Tool running';
    case 'waiting':
      return '◐ Waiting for permission';
    case 'error':
      return '! Error';
    default:
      return '○ Idle';
  }
}

export function healthLabel(health?: string): string {
  switch (health) {
    case 'healthy':
      return 'Healthy';
    case 'moderate':
      return 'Moderate';
    case 'high':
      return 'High — consider compacting';
    case 'critical':
      return 'Critical — compact now';
    default:
      return '';
  }
}
