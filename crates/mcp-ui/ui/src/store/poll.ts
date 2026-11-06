// The `app_snapshot` poll: every second while this card's own lane starts, every 3
// seconds otherwise, every 10 while no daemon runs. It pauses while the card is hidden
// or scrolled out of the chat, stops after 15 minutes without a click or key, and stops
// for good once a newer card for the same lane is in the chat.

import type { Snapshot } from '../types/results';
import { isBusy } from '../utils/state';
import {
  bridge,
  card,
  currentView,
  cwd,
  daemonStopped,
  isFullscreen,
  resultLane,
  snapshotLanes,
  starting,
  structured,
  update
} from './card';

/** Poll cadence while something of this card starts, and otherwise. */
const FAST_MS = 1000;
const SLOW_MS = 3000;
/** Poll cadence while no daemon runs: the card never starts one, it waits for one. */
const STOPPED_MS = 10_000;
/** Polling stops after this long without a click or a key press in the card. */
export const IDLE_MS = 15 * 60 * 1000;
/** Failed snapshots in a row before polling gives up until the next click. */
const MAX_FAILURES = 5;

let timer = 0;
let inFlight = false;
let again = false;
let failures = 0;
let tornDown = false;
let lastInteraction = Date.now();
let onScreen = true;

/** Whether a result's data lists no lane and no box, which an empty board shows once. */
function emptyBoard(): boolean {
  const data = structured()?.data || {};
  return !(data.items || []).length && !(data.hosts || []).length;
}

/**
 * The `app_snapshot` arguments of the current view, or null when it does not poll.
 * Only the fullscreen board draws CPU, so only it asks for metrics.
 */
function snapshotArgs(): Record<string, unknown> | null {
  if (card.superseded) return null;
  switch (currentView()) {
    case 'board':
      return card.result && emptyBoard() ? null : { cwd: cwd(), metrics: isFullscreen() };
    case 'lane':
    case 'failure':
      return { cwd: cwd(), lane: resultLane(structured()) };
    case 'clean':
      // Read-only servers have no clean_plan, so the planner shows the result alone.
      return structured()?.read_only ? null : { cwd: cwd() };
    default:
      return null;
  }
}

/**
 * Whether something this card is waiting for is on its way: its own start, an action
 * it runs, or a lane it started. Other agents' lanes starting on the board do not count.
 */
function isFast(): boolean {
  if (starting() || Object.keys(card.busy).length) return true;
  const busy = (ref: string) => {
    const lane = snapshotLanes().find((l) => l.reference === ref);
    return !!lane && (isBusy(lane.state) || lane.units.some((unit) => isBusy(unit.state)));
  };
  if (currentView() === 'lane') {
    const ref = resultLane(structured());
    return !!ref && busy(ref);
  }
  return card.started.some(busy);
}

/** Runs the next poll after `delay`; one runs at a time. */
function schedule(delay: number): void {
  clearTimeout(timer);
  if (!tornDown && !card.superseded) timer = window.setTimeout(tick, delay);
}

/** How long until the next poll. */
function every(): number {
  return isFast() ? FAST_MS : daemonStopped() ? STOPPED_MS : SLOW_MS;
}

/**
 * Whether the snapshot names a newer card for this card's lane. A card counts only
 * once its own result carries a number.
 */
function supersededBy(snapshot: Snapshot | null): boolean {
  const own = structured()?.card;
  const lane = resultLane(structured());
  const newest = lane ? snapshot?.newest_card?.[lane] : undefined;
  return typeof own === 'number' && typeof newest === 'number' && newest > own;
}

/** One `app_snapshot`, then the next one, unless hidden, idle or failing. */
async function tick(): Promise<void> {
  if (inFlight) {
    again = true;
    return;
  }
  const args = snapshotArgs();
  if (!args) return;
  if (document.visibilityState === 'hidden' || !onScreen) return update({ poll: 'paused' });
  if (Date.now() - lastInteraction > IDLE_MS) return update({ poll: 'idle' });
  inFlight = true;
  try {
    const result = await bridge.callTool<Snapshot>('app_snapshot', args);
    const snapshot = result.structuredContent || null;
    failures = 0;
    if (supersededBy(snapshot)) {
      clearTimeout(timer);
      update({ superseded: true, poll: 'off' });
      return;
    }
    const started = card.started.filter((ref) =>
      (snapshot?.status?.items || []).some((lane) => lane.reference === ref && isBusy(lane.state))
    );
    update({
      snapshot,
      snapshotSeq: card.snapshotSeq + 1,
      poll: 'live',
      pollError: '',
      updatedAt: Date.now(),
      // A lane it started stays fast only while it starts.
      started: Object.keys(card.busy).length ? card.started : started
    });
  } catch (error) {
    failures++;
    update({
      poll: failures >= MAX_FAILURES ? 'error' : card.poll,
      pollError: String((error as Error).message || error)
    });
  } finally {
    inFlight = false;
  }
  if (failures >= MAX_FAILURES || card.superseded) return;
  schedule(again ? 0 : every());
  again = false;
}

/** Polls again after a pause, without counting it as a click. */
function resume(): void {
  if (card.poll !== 'paused' || document.visibilityState === 'hidden' || !onScreen) return;
  card.poll = 'live';
  schedule(0);
}

/** Polls now, also after polling stopped; a superseded card stays stopped. */
export function pollNow(): void {
  if (card.superseded) return;
  lastInteraction = Date.now();
  failures = 0;
  if (card.poll === 'error' || card.poll === 'idle' || card.poll === 'paused') {
    update({ poll: 'live' });
  }
  schedule(0);
}

/** Stops polling for good, before the host tears the card down. */
export function stopPolling(): void {
  tornDown = true;
  clearTimeout(timer);
  update({ poll: 'off' });
}

/**
 * Watches what pauses and resumes the poll: the page's visibility, the card
 * scrolling in and out of the chat, and clicks or keys that count as activity.
 */
export function watchActivity(): void {
  document.addEventListener('visibilitychange', resume);
  // In a frame, the implicit root is the host's viewport, so this sees the
  // card scrolled out of the chat.
  new IntersectionObserver((entries) => {
    onScreen = entries[entries.length - 1]?.isIntersecting ?? onScreen;
    resume();
  }).observe(document.body);
  for (const type of ['pointerdown', 'keydown']) {
    document.addEventListener(
      type,
      () => {
        lastInteraction = Date.now();
        if (card.poll === 'idle') pollNow();
      },
      true
    );
  }
}
