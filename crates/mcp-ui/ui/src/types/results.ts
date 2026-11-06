// What the devlanes MCP server sends the card: the tool result envelope of the
// `devlanes` and `devlanes_read` tools, and the results of the app-only tools.

import type { LogsPage, StatusPage, TopPage } from './proto';
import type { RemoteList } from './remote';

/** The card a result asks for; null renders nothing. */
export type View = 'lane' | 'board' | 'failure' | 'clean' | 'pair' | null;

/** `structuredContent` of a `devlanes` or `devlanes_read` result. */
export interface Envelope {
  /** The subcommand path: `start`, `lane clean`, `daemon pair`. */
  command?: string;
  /** The arguments the server ran, after it added `--timeout` or `--yes`. */
  args?: string[];
  cwd?: string | null;
  exit_code?: number;
  view?: View;
  /** The command's JSON output: one document, an array for NDJSON, or null. */
  data?: any;
  read_only?: boolean;
  data_truncated?: boolean;
  /**
   * This card's number for its lane: the server counts the results that name one lane,
   * per lane, so a higher number in `Snapshot.newest_card` means a newer card exists.
   * Null for results about no lane or several.
   */
  card?: number | null;
}

/** One block of a tool result's `content`. */
export interface ContentBlock {
  type: string;
  text?: string;
}

/** A `tools/call` result. */
export interface ToolResult<T = any> {
  content?: ContentBlock[];
  structuredContent?: T;
  isError?: boolean;
}

/** The arguments of the model's tool call, from `ui/notifications/tool-input`. */
export interface ToolInput {
  args?: string[];
  cwd?: string;
}

/** `app_snapshot`'s result; `daemon` is `stopped` when no daemon runs. */
export interface Snapshot {
  status: StatusPage | null;
  top: TopPage | null;
  hosts: RemoteList | null;
  daemon?: 'stopped';
  read_only?: boolean;
  /** The newest card number of each lane in `status`. */
  newest_card?: Record<string, number>;
}

/** `app_logs`'s result. */
export interface LogsResult {
  logs?: LogsPage;
}

/** `app_action`'s result. */
export interface ActionResult {
  exit_code: number;
  text?: string;
  data?: any;
}

/** The actions `app_action` takes. */
export type Action = 'start' | 'stop' | 'restart' | 'reload' | 'clean' | 'clean_plan' | 'pair';
