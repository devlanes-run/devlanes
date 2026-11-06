// The daemon's data as the card reads it: subsets of the types in crates/proto
// with the same field names. Fields older daemons omit are optional.

/** Who started a unit (`Actor`). */
export interface Actor {
  kind: 'human' | 'agent';
  agent_id?: string | null;
  session_id?: string | null;
}

/** A machine-readable error or reason (`RpcError`, or a `why` reason). */
export interface RpcError {
  code: string;
  data?: Record<string, any> | null;
  exit_code?: number;
}

/** One unit of a lane (`UnitSummary`). */
export interface UnitSummary {
  name: string;
  lane?: string;
  state: string;
  exit_code?: number | null;
  ports?: Record<string, number>;
  urls?: string[];
  actor?: Actor | null;
  reason?: RpcError | null;
  kind?: string;
  hidden?: boolean;
  started_at_ms?: number | null;
  finished_at_ms?: number | null;
  display_name?: string | null;
  ready_check?: string | null;
  role?: string | null;
  public_url?: string | null;
  power?: string | null;
}

/** An idle lane's power level (`LanePower`). */
export interface LanePower {
  level: string;
  since_ms: number;
}

/** A container a lane uses (`ContainerSummary`). */
export interface ContainerSummary {
  name: string;
  state?: string | null;
}

/** A condition that needs a look without stopping the lane (`LaneWarning`). */
export interface LaneWarning {
  code: string;
  data?: { message?: string } | null;
}

/** One lane (`LaneSummary`). */
export interface LaneSummary {
  reference: string;
  project: string;
  index?: number | null;
  branch?: string | null;
  name?: string | null;
  path?: string;
  state: string;
  config_changed?: boolean;
  units: UnitSummary[];
  domain?: string | null;
  needs_env?: boolean;
  containers?: ContainerSummary[];
  warnings?: LaneWarning[];
  power?: LanePower | null;
}

/** A box this machine links to, as `status` lists it under `hosts`. */
export interface HostLink {
  host: string;
  state: string;
  error?: string | null;
  fix?: string | null;
}

/** `devlanes status --json`. */
export interface StatusPage {
  items: LaneSummary[];
  hosts?: HostLink[];
}

/** One unit that failed to start, in a start result. */
export interface StartFailure {
  lane: string;
  unit: string;
  exit_code?: number | null;
  reason?: RpcError | null;
  log_tail?: string[];
}

/** `devlanes start --json` and its siblings (`StartResult`). */
export interface StartResult {
  lane: string;
  branch?: string | null;
  ready?: boolean;
  units: UnitSummary[];
  failures?: StartFailure[];
  warnings?: LaneWarning[];
}

/** One log line (`LogLine`); `seq` is null for lines that came without a cursor. */
export interface LogLine {
  unit: string;
  seq: number | null;
  text: string;
  level: string | null;
}

/** A page of `devlanes logs --json`. */
export interface LogsPage {
  items?: LogLine[];
  next_cursor?: string | null;
}

/** A port another process holds, in `debug errors`. */
export interface PortConflict {
  port: number;
  pid?: number | null;
}

/** One unit of `devlanes debug errors --json`. */
export interface ErrorsItem {
  lane: string;
  unit: string;
  state?: string;
  exit_code?: number | null;
  crashes?: number;
  reason?: RpcError | null;
  error_lines: string[];
  port_conflicts?: PortConflict[];
}

/** `devlanes debug why --json`. */
export interface WhyResult {
  lane?: string;
  unit: string;
  state?: string;
  reasons: RpcError[];
}

/** One row of `devlanes debug top --json`: `history` holds `[time_ms, cpu_percent, ...]`. */
export interface TopRow {
  lane: string;
  unit?: string;
  ended?: boolean;
  history?: number[][];
}

/** `devlanes debug top --json`. */
export interface TopPage {
  lanes?: TopRow[];
  units?: TopRow[];
}

/** `devlanes daemon pair --json`. */
export interface Pairing {
  code?: string;
  valid_ms?: number;
}
