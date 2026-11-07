// The card's state and what the views derive from it. One mutable object and a
// redraw callback, so the bridge handlers, the poll and the views all read the
// same state without a context provider.

import { Bridge, type HostContext } from '../bridge';
import type { LaneSummary, StartResult, StatusPage } from '../types/proto';
import type { Envelope, Snapshot, ToolInput, ToolResult, View } from '../types/results';
import { commandOf } from '../utils/format';
import { isBusy } from '../utils/state';

/** Commands whose card shows a lane while it starts. */
const STARTS = ['start', 'restart', 'reload', 'lane resume'];

/** Whether the card polls: live, paused while hidden, idle after 15 minutes, or given up. */
export type PollState = 'off' | 'live' | 'paused' | 'idle' | 'error';

/** A line above the buttons, for an action that failed. */
export interface NoteState {
  text: string;
  tone?: string;
}

/** The view to render, plus `pending` for a call that has not answered yet. */
export type CurrentView = View | 'pending';

/** Everything the views render from. */
export interface CardState {
  connected: boolean;
  /** Why the handshake failed. */
  failed: string | null;
  context: HostContext;
  input: ToolInput | null;
  inputDone: boolean;
  result: ToolResult<Envelope> | null;
  /** When the result arrived, so the pairing countdown needs no clock shared with a box. */
  resultAt: number;
  cancelled: string | null;
  snapshot: Snapshot | null;
  /** Counts snapshots, so the log viewer reads new lines on each one. */
  snapshotSeq: number;
  poll: PollState;
  pollError: string;
  /** When the data on screen was last fresh, for "updated 14:02" once polling stops. */
  updatedAt: number;
  /** A newer card for this lane is in the chat, so this one stopped and shrank. */
  superseded: boolean;
  /** The lines sent as model context. */
  actions: string[];
  /** The action running for a lane or unit reference. */
  busy: Record<string, string>;
  /** Lanes this card started or restarted, which the board polls fast for. */
  started: string[];
  note: NoteState | null;
}

export const bridge = new Bridge(window.parent);

export const card: CardState = {
  connected: false,
  failed: null,
  context: {},
  input: null,
  inputDone: false,
  result: null,
  resultAt: 0,
  cancelled: null,
  snapshot: null,
  snapshotSeq: 0,
  poll: 'off',
  pollError: '',
  updatedAt: 0,
  superseded: false,
  actions: [],
  busy: {},
  started: [],
  note: null
};

let redraw = () => {};

/** Sets the function that renders the card after each change. */
export function onChange(fn: () => void): void {
  redraw = fn;
}

/** Merges a change into the state and renders. */
export function update(patch: Partial<CardState>): void {
  Object.assign(card, patch);
  redraw();
}

/** The tool result's `structuredContent`, null until it arrives. */
export function structured(): Envelope | null {
  return card.result?.structuredContent || null;
}

/** Whether the server runs with `--read-only`, so the card shows no button that changes anything. */
export function readOnly(): boolean {
  return !!(structured()?.read_only || card.snapshot?.read_only);
}

/** Whether no daemon runs. A start in flight is about to start one, so it does not count. */
export function daemonStopped(): boolean {
  return card.snapshot?.daemon === 'stopped' && !starting();
}

/** Whether the agent's own tool call is still running: its input came, its result not yet. */
export function callRunning(): boolean {
  return !!card.input && !card.result && !card.cancelled;
}

/**
 * Whether the card may hand something to the agent. Not while the agent's own call
 * runs, which would post a second request into a turn that has not finished.
 */
export function canHandOff(): boolean {
  return !callRunning() && !card.superseded;
}

/** Whether a start, restart, reload or resume runs and has not answered yet. */
export function starting(): boolean {
  return !card.result && STARTS.includes(commandOf(card.input?.args));
}

/** Whether the host offers fullscreen, where the log viewer and board management go. */
export function canFullscreen(): boolean {
  return (card.context.availableDisplayModes || []).includes('fullscreen');
}

/** Whether the card is shown fullscreen. */
export function isFullscreen(): boolean {
  return card.context.displayMode === 'fullscreen';
}

/** Whether polling stopped for good (idle or failing), so the card shows when it was fresh. */
export function pollStopped(): boolean {
  return card.poll === 'idle' || card.poll === 'error';
}

/** The working directory the tool ran in, so app tools resolve the same lane. */
export function cwd(): string | undefined {
  return structured()?.cwd || card.input?.cwd || undefined;
}

/** The view to render: the result's `view`, or a guess from the arguments while the call runs. */
export function currentView(): CurrentView {
  if (card.result) return structured()?.view ?? null;
  if (!card.input) return 'pending';
  return STARTS.includes(commandOf(card.input.args)) ? 'lane' : 'pending';
}

/** The lanes of the latest snapshot. */
export function snapshotLanes(): LaneSummary[] {
  return card.snapshot?.status?.items || [];
}

/**
 * The lane a result is about: a start result's `lane`, a status page's only
 * lane, or the lane of a failure.
 */
export function resultLane(sc: Envelope | null): string | undefined {
  const data = sc?.data;
  if (!data) return undefined;
  if (typeof data.lane === 'string') return data.lane;
  const items: any[] = Array.isArray(data.items) ? data.items : [];
  if (items.length === 1 && items[0].reference) return items[0].reference;
  if (Array.isArray(data.failures) && data.failures[0]) return data.failures[0].lane;
  if (items[0] && typeof items[0].lane === 'string') return items[0].lane;
  return undefined;
}

/**
 * A LaneSummary built from the tool result until a snapshot arrives: a status
 * page's lane as is, a start result as a lane of the units it started.
 */
function laneFromResult(sc: Envelope | null): LaneSummary | null {
  const data = sc?.data as (Partial<StatusPage> & Partial<StartResult>) | null | undefined;
  if (!data) return null;
  if (Array.isArray(data.items) && data.items[0]?.reference) return data.items[0];
  if (typeof data.lane !== 'string' || !Array.isArray(data.units)) return null;
  const failed = (data.failures || []).length > 0;
  const [project = '', index] = data.lane.split('@');
  return {
    reference: data.lane,
    project,
    index: Number(index),
    branch: data.branch ?? null,
    state: failed ? 'failed' : data.ready ? 'running' : 'starting',
    units: data.units,
    containers: [],
    warnings: data.warnings || []
  };
}

/**
 * Finds a lane in the latest snapshot, else in the tool result. Without a
 * reference (a start still running) it picks the lane of the working
 * directory, else the one starting.
 */
export function findLane(ref: string | undefined): LaneSummary | null {
  const lanes = snapshotLanes();
  if (ref) return lanes.find((lane) => lane.reference === ref) || laneFromResult(structured());
  const normal = (path: string) => path.replace(/\\/g, '/').replace(/\/$/, '').toLowerCase();
  const dir = normal(cwd() || '');
  const here = (lane: LaneSummary) => normal(String(lane.path || '')) === dir;
  return (dir && lanes.find(here)) || lanes.find((lane) => isBusy(lane.state)) || null;
}
