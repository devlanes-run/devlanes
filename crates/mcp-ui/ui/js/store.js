// @ts-check
// The card's state, the snapshot poll and the actions every view shares.
import { html } from '../vendor/standalone.module.js';
import { Bridge } from './bridge.js';
import { Button } from './ui.js';
import { commandOf, isBusy } from './util.js';

/** Poll cadence while something starts, and otherwise. */
const FAST_MS = 1000;
const SLOW_MS = 3000;
/** Poll cadence while no daemon runs: the card never starts one, it waits for one. */
const STOPPED_MS = 10000;
/** Polling stops after this long without a click or a key press in the card. */
const IDLE_MS = 15 * 60 * 1000;
/** Failed snapshots in a row before polling gives up until the next click. */
const MAX_FAILURES = 5;
/** Commands whose card shows a lane while it starts. */
const STARTS = ['start', 'restart', 'reload', 'lane resume'];
/** How long a note under the card stays. */
const NOTE_MS = 6000;

export const bridge = new Bridge(window.parent);

/**
 * Everything the views render from. `result` is the tool result, `snapshot`
 * the latest `app_snapshot` (`{status, top}`), `drill` a lane or unit opened
 * from inside the card, `actions` the lines sent as model context.
 */
export const card = {
  connected: false,
  /** @type {string | null} */
  failed: null,
  /** @type {import('./bridge.js').HostContext} */
  context: {},
  /** @type {any} */
  input: null,
  inputDone: false,
  /** @type {any} */
  result: null,
  resultAt: 0,
  /** @type {string | null} */
  cancelled: null,
  /** @type {any} */
  snapshot: null,
  snapshotSeq: 0,
  /** @type {'off' | 'live' | 'paused' | 'idle' | 'error'} */
  poll: 'off',
  pollError: '',
  /** @type {{lane?: string, unit?: string} | null} */
  drill: null,
  /** @type {string[]} */
  actions: [],
  /** @type {Record<string, string>} */
  busy: {},
  /** @type {{text: string, tone?: string} | null} */
  note: null,
};

let redraw = () => {};
let timer = 0;
let inFlight = false;
let again = false;
let failures = 0;
let tornDown = false;
let lastInteraction = Date.now();
let onScreen = true;

/**
 * Sets the function that renders the card after each change.
 * @param {() => void} fn
 */
export function onChange(fn) {
  redraw = fn;
}

/**
 * Merges a change into the state and renders.
 * @param {Partial<typeof card>} patch
 */
export function update(patch) {
  Object.assign(card, patch);
  redraw();
}

/** The tool result's `structuredContent`, null until it arrives. */
export function structured() {
  return card.result ? card.result.structuredContent || null : null;
}

/** Whether the server runs with `--read-only`, so the card shows no button that changes anything. */
export function readOnly() {
  const sc = structured();
  return !!((sc && sc.read_only) || (card.snapshot && card.snapshot.read_only));
}

/** Whether no daemon runs. A start in flight is about to start one, so it does not count. */
export function daemonStopped() {
  return !!card.snapshot && card.snapshot.daemon === 'stopped' && !starting();
}

/** Whether a start, restart, reload or resume runs and has not answered yet. */
function starting() {
  return !card.result && STARTS.includes(commandOf(card.input && card.input.args));
}

/** The working directory the tool ran in, so app tools resolve the same lane. */
export function cwd() {
  const sc = structured();
  return (sc && sc.cwd) || (card.input && card.input.cwd) || undefined;
}

/**
 * The view to render: the result's `view`, or a guess from the arguments while
 * the call runs. `pending` is a card that only shows the command.
 * @returns {string | null}
 */
export function currentView() {
  if (card.result) {
    const sc = structured();
    return sc ? sc.view ?? null : null;
  }
  if (!card.input) return 'pending';
  return STARTS.includes(commandOf(card.input.args)) ? 'lane' : 'pending';
}

/**
 * The lane a result is about: a start result's `lane`, a status page's only
 * lane, or the lane of a failure.
 * @param {any} sc
 * @returns {string | undefined}
 */
export function resultLane(sc) {
  const data = sc && sc.data;
  if (!data) return undefined;
  if (typeof data.lane === 'string') return data.lane;
  const items = Array.isArray(data.items) ? data.items : [];
  if (items.length === 1 && items[0].reference) return items[0].reference;
  if (Array.isArray(data.failures) && data.failures[0]) return data.failures[0].lane;
  if (items[0] && typeof items[0].lane === 'string') return items[0].lane;
  return undefined;
}

/**
 * A LaneSummary built from the tool result until a snapshot arrives: a status
 * page's lane as is, a start result as a lane of the units it started.
 * @param {any} sc
 */
function laneFromResult(sc) {
  const data = sc && sc.data;
  if (!data) return null;
  if (Array.isArray(data.items) && data.items[0] && data.items[0].reference) return data.items[0];
  if (typeof data.lane !== 'string' || !Array.isArray(data.units)) return null;
  const failed = (data.failures || []).length > 0;
  return {
    reference: data.lane,
    project: data.lane.split('@')[0],
    index: Number(data.lane.split('@')[1]),
    branch: data.branch,
    state: failed ? 'failed' : data.ready ? 'running' : 'starting',
    units: data.units,
    containers: [],
    warnings: data.warnings || [],
  };
}

/**
 * Finds a lane in the latest snapshot, else in the tool result. Without a
 * reference (a start still running) it picks the lane of the working
 * directory, else the one starting.
 * @param {string | undefined} ref
 */
export function findLane(ref) {
  const lanes = (card.snapshot && card.snapshot.status && card.snapshot.status.items) || [];
  if (ref) return lanes.find((lane) => lane.reference === ref) || laneFromResult(structured());
  const dir = (cwd() || '').replace(/\\/g, '/').replace(/\/$/, '').toLowerCase();
  const here = (lane) => String(lane.path || '').replace(/\\/g, '/').replace(/\/$/, '').toLowerCase() === dir;
  return (dir && lanes.find(here)) || lanes.find((lane) => isBusy(lane.state)) || null;
}

/**
 * The `app_snapshot` arguments of the current view, or null when it does not
 * poll. Only the lane card and the board draw CPU, so only they ask for metrics.
 */
function snapshotArgs() {
  const view = currentView();
  if (card.drill && card.drill.unit) return { cwd: cwd(), lane: card.drill.unit.split(':')[0] };
  if (view === 'board') return { cwd: cwd(), metrics: true };
  if (view === 'lane') return { cwd: cwd(), lane: resultLane(structured()), metrics: true };
  if (view === 'failure') return { cwd: cwd(), lane: resultLane(structured()) };
  return null;
}

/** Whether anything is on its way somewhere, which polls every second. */
function isFast() {
  if (starting()) return true;
  if (Object.keys(card.busy).length) return true;
  const lanes = (card.snapshot && card.snapshot.status && card.snapshot.status.items) || [];
  return lanes.some((lane) => isBusy(lane.state) || (lane.units || []).some((unit) => isBusy(unit.state)));
}

/**
 * Runs the next poll after `delay`; one runs at a time.
 * @param {number} delay
 */
function schedule(delay) {
  clearTimeout(timer);
  if (!tornDown) timer = window.setTimeout(tick, delay);
}

/** How long until the next poll. */
function every() {
  return isFast() ? FAST_MS : daemonStopped() ? STOPPED_MS : SLOW_MS;
}

/** One `app_snapshot`, then the next one, unless hidden, idle or failing. */
async function tick() {
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
    const result = await bridge.callTool('app_snapshot', args);
    failures = 0;
    update({ snapshot: result.structuredContent || null, snapshotSeq: card.snapshotSeq + 1, poll: 'live', pollError: '' });
  } catch (error) {
    failures++;
    update({ poll: failures >= MAX_FAILURES ? 'error' : card.poll, pollError: String(/** @type {Error} */ (error).message || error) });
  } finally {
    inFlight = false;
  }
  if (failures >= MAX_FAILURES) return;
  schedule(again ? 0 : every());
  again = false;
}

/** Polls again after a pause, without counting it as a click. */
function resume() {
  if (card.poll !== 'paused' || document.visibilityState === 'hidden' || !onScreen) return;
  card.poll = 'live';
  schedule(0);
}

/** Polls now, also after polling stopped. */
export function pollNow() {
  lastInteraction = Date.now();
  failures = 0;
  if (card.poll === 'error' || card.poll === 'idle' || card.poll === 'paused') card.poll = 'live';
  schedule(0);
}

/** Stops polling for good, before the host tears the card down. */
export function stopPolling() {
  tornDown = true;
  clearTimeout(timer);
  update({ poll: 'off' });
}

document.addEventListener('visibilitychange', resume);
// In a frame, the implicit root is the host's viewport, so this sees the
// card scrolled out of the chat.
new IntersectionObserver((entries) => {
  onScreen = entries[entries.length - 1].isIntersecting;
  resume();
}).observe(document.body);
for (const type of ['pointerdown', 'keydown']) {
  document.addEventListener(
    type,
    () => {
      lastInteraction = Date.now();
      if (card.poll === 'idle') pollNow();
    },
    true,
  );
}

/**
 * Adds a line to what the model reads on its next turn. The host keeps only the
 * latest update, so each one carries the last few lines.
 * @param {string} line
 */
export function tellModel(line) {
  const actions = [...card.actions, line].slice(-10);
  update({ actions });
  bridge.updateModelContext(actions.join('\n')).catch(() => {});
}

/**
 * Runs `app_action` for a button, tells the model what the person did and polls.
 * @param {string} action start, stop, restart, reload, clean, clean_plan or pair
 * @param {string | undefined} target a lane or unit reference
 * @param {string} [done] what happened, in the past tense, for the model: `stopped acme@1:api`
 * @returns {Promise<any>} the action's `{exit_code, text, data}`, or null when it failed
 */
export async function act(action, target, done) {
  const key = target || action;
  update({ busy: { ...card.busy, [key]: action }, note: null });
  try {
    // Not callTool: a command that exits nonzero comes back with isError, and
    // its structured content still says what happened.
    /** @type {import('./bridge.js').ToolResult} */
    const result = await bridge.request('tools/call', { name: 'app_action', arguments: { cwd: cwd(), action, target } });
    const out = result.structuredContent || { exit_code: 1, text: (result.content || []).map((b) => b.text || '').join('\n') };
    const first = String(out.text || '').trim().split('\n')[0];
    if (done) tellModel(`user ${done} from the card${out.exit_code ? `; it exited ${out.exit_code}: ${first}` : ''}`);
    if (out.exit_code) flash({ text: first || `${action} exited ${out.exit_code}`, tone: 'failure' });
    return out;
  } catch (error) {
    flash({ text: `${action} ${target || ''}: ${/** @type {Error} */ (error).message}`, tone: 'failure' });
    return null;
  } finally {
    const busy = { ...card.busy };
    delete busy[key];
    update({ busy });
    if (action !== 'clean_plan' && action !== 'pair') pollNow();
  }
}

/**
 * Shows a note under the card for a few seconds.
 * @param {{text: string, tone?: string}} note
 */
export function flash(note) {
  update({ note });
  setTimeout(() => {
    if (card.note === note) update({ note: null });
  }, NOTE_MS);
}

/**
 * Sends a message to the agent as the person.
 * @param {string} text
 */
export function ask(text) {
  bridge
    .sendMessage(text)
    .then(() => flash({ text: 'sent to the agent' }))
    .catch((error) => flash({ text: `not sent: ${error.message}`, tone: 'failure' }));
}

/**
 * Opens a URL through the host.
 * @param {string} url
 */
export function openUrl(url) {
  bridge.openLink(url).catch((error) => flash({ text: `not opened: ${error.message}`, tone: 'failure' }));
}

/** The fullscreen toggle, shown only where the host offers fullscreen. */
export function FullscreenButton() {
  const modes = card.context.availableDisplayModes || [];
  if (!modes.includes('fullscreen')) return null;
  const full = card.context.displayMode === 'fullscreen';
  const toggle = async () => {
    try {
      const mode = await bridge.requestDisplayMode(full ? 'inline' : 'fullscreen');
      update({ context: { ...card.context, displayMode: mode } });
      document.documentElement.classList.toggle('fullscreen', mode === 'fullscreen');
    } catch (error) {
      flash({ text: `no fullscreen: ${/** @type {Error} */ (error).message}`, tone: 'failure' });
    }
  };
  return html`<${Button} icon=${full ? 'inline' : 'fullscreen'} label=${full ? 'exit fullscreen' : 'fullscreen'} iconOnly onClick=${toggle} />`;
}

/** Whether the card polls now, and a way back when it stopped. */
export function PollStatus() {
  switch (card.poll) {
    case 'live':
      if (card.pollError) return html`<span class="poll tone-attention">retrying: ${card.pollError}</span>`;
      if (daemonStopped()) return html`<span class="poll">checking every ${every() / 1000}s</span>`;
      return html`<span class="poll"><span class="tone-ok" aria-hidden="true">●</span> live · every ${every() / 1000}s</span>`;
    case 'paused':
      return html`<span class="poll">paused while hidden</span>`;
    case 'idle':
      return html`<span class="poll">stopped after 15m idle <${Button} label="resume" onClick=${pollNow} /></span>`;
    case 'error':
      return html`<span class="poll tone-failure">can't reach devlanes: ${card.pollError} <${Button} label="retry" onClick=${pollNow} /></span>`;
    default:
      return html`<span class="poll">${card.pollError && `retrying: ${card.pollError}`}</span>`;
  }
}
