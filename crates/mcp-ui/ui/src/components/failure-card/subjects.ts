import { card, snapshotLanes } from '../../store/card';
import type {
  ErrorsItem,
  LogLine,
  PortConflict,
  RpcError,
  StartFailure,
  UnitSummary
} from '../../types/proto';
import type { Envelope } from '../../types/results';

/** A unit the failure card is about, with what the tool result says of it. */
export interface Subject {
  lane?: string | undefined;
  unit: string;
  state?: string | undefined;
  exit?: number | null | undefined;
  crashes?: number | undefined;
  reasons?: RpcError[];
  /** Plain lines without cursors, from a failure or an errors item. */
  tail?: string[] | undefined;
  /** Log lines the tool result already carries. */
  lines?: LogLine[];
  /** The cursor before those lines. */
  next?: string | null | undefined;
  conflicts?: PortConflict[] | undefined;
}

/** The units a result is about, in the order it lists them. */
export function subjectsOf(sc: Envelope | null): Subject[] {
  if (card.drill?.unit) {
    const [lane, unit = ''] = card.drill.unit.split(':');
    return [{ lane, unit }];
  }
  const data = sc?.data || {};
  if (Array.isArray(data.reasons)) {
    return [{ lane: data.lane, unit: data.unit, state: data.state, reasons: data.reasons }];
  }
  if (Array.isArray(data.failures) && data.failures.length) {
    return data.failures.map((f: StartFailure) => ({
      lane: f.lane,
      unit: f.unit,
      exit: f.exit_code,
      reasons: f.reason ? [f.reason] : [],
      tail: f.log_tail
    }));
  }
  if (data.lane && Array.isArray(data.units) && data.units[0] && 'lines' in data.units[0]) {
    return data.units.map((group: { unit: string; lines: LogLine[] }) => ({
      lane: data.lane,
      unit: group.unit,
      lines: group.lines
    }));
  }
  const items: any[] = Array.isArray(data.items) ? data.items : [];
  if (items[0] && 'error_lines' in items[0]) {
    return items.map((e: ErrorsItem) => ({
      lane: e.lane,
      unit: e.unit,
      state: e.state,
      exit: e.exit_code,
      crashes: e.crashes,
      reasons: e.reason ? [e.reason] : [],
      tail: e.error_lines,
      conflicts: e.port_conflicts
    }));
  }
  if (items[0] && 'seq' in items[0]) {
    // Log lines name their unit only; a lane the command named (`acme@2:api`) goes with it.
    const named = (sc?.args || []).map((arg) => /^([^\s:]+@\d+):/.exec(arg)).find(Boolean);
    const lines = items as LogLine[];
    const units = [...new Set(lines.map((line) => line.unit))];
    return units.map((unit) => ({
      lane: named ? named[1] : undefined,
      unit,
      lines: lines.filter((line) => line.unit === unit),
      next: units.length === 1 ? data.next_cursor : null
    }));
  }
  return [];
}

/** The unit's latest summary from the snapshot, when the poll has one. */
export function liveUnit(subject: Subject): UnitSummary | undefined {
  const lanes = snapshotLanes();
  const lane =
    lanes.find((l) => l.reference === subject.lane) || (lanes.length === 1 ? lanes[0] : undefined);
  return lane?.units.find((unit) => unit.name === subject.unit);
}
