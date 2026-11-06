// The `app_snapshot` poll: every second while something starts, every 3 seconds
// otherwise, every 10 while no daemon runs. It pauses while the card is hidden
// or scrolled out of the chat and stops after 15 minutes without a click or key.

import type { Snapshot } from '../types/results';
import { isBusy } from '../utils/state';
import {
  bridge,
  card,
  currentView,
  cwd,
  daemonStopped,
  resultLane,
  snapshotLanes,
  starting,
  structured,
  update
} from './card';

/** Poll cadence while something starts, and otherwise. */
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

/**
 * The `app_snapshot` arguments of the current view, or null when it does not
 * poll. Only the lane card and the board draw CPU, so only they ask for metrics.
 */
function snapshotArgs(): Record<string, unknown> | null {
  const view = currentView();
  if (card.drill?.unit) return { cwd: cwd(), lane: card.drill.unit.split(':')[0] };
  if (view === 'board') return { cwd: cwd(), metrics: true };
  if (view === 'lane') return { cwd: cwd(), lane: resultLane(structured()), metrics: true };
  if (view === 'failure') return { cwd: cwd(), lane: resultLane(structured()) };
  return null;
}

/** Whether anything is on its way somewhere, which polls every second. */
function isFast(): boolean {
  if (starting()) return true;
  if (Object.keys(card.busy).length) return true;
  return snapshotLanes().some(
    (lane) => isBusy(lane.state) || (lane.units || []).some((unit) => isBusy(unit.state))
  );
}

/** Runs the next poll after `delay`; one runs at a time. */
function schedule(delay: number): void {
  clearTimeout(timer);
  if (!tornDown) timer = window.setTimeout(tick, delay);
}

/** How long until the next poll. */
export function every(): number {
  return isFast() ? FAST_MS : daemonStopped() ? STOPPED_MS : SLOW_MS;
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
    failures = 0;
    update({
      snapshot: result.structuredContent || null,
      snapshotSeq: card.snapshotSeq + 1,
      poll: 'live',
      pollError: ''
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
  if (failures >= MAX_FAILURES) return;
  schedule(again ? 0 : every());
  again = false;
}

/** Polls again after a pause, without counting it as a click. */
function resume(): void {
  if (card.poll !== 'paused' || document.visibilityState === 'hidden' || !onScreen) return;
  card.poll = 'live';
  schedule(0);
}

/** Polls now, also after polling stopped. */
export function pollNow(): void {
  lastInteraction = Date.now();
  failures = 0;
  if (card.poll === 'error' || card.poll === 'idle' || card.poll === 'paused') card.poll = 'live';
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
