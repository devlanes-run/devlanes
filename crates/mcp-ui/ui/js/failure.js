// @ts-check
// The failure card: why a unit failed, who started it, and a log viewer with
// cursors, grep, "load earlier" and a picked line the agent can read around.
// Shown for failed starts, debug why, debug errors and logs, and for a unit
// opened from a lane card.
import { html, useEffect, useLayoutEffect, useRef, useState } from '../vendor/standalone.module.js';
import { Button, Glyph, Icon, Note, Window } from './ui.js';
import { TextView } from './lane.js';
import { FullscreenButton, PollStatus, act, ask, bridge, card, cwd, readOnly, structured, update } from './store.js';
import { actorLabel, reasonText, since, unitRef, unitState } from './util.js';

/** Lines the viewer keeps; older ones drop off the top while it follows. */
const MAX_LINES = 2000;
/** Lines each fetch asks for. */
const PAGE = 200;

/**
 * @typedef {object} Subject
 * @property {string} [lane]
 * @property {string} unit
 * @property {string} [state]
 * @property {number | null} [exit]
 * @property {number} [crashes]
 * @property {any[]} [reasons]
 * @property {string[]} [tail] plain lines without cursors, from a failure or an errors item
 * @property {any[]} [lines] LogLines the tool result already carries
 * @property {string | null} [next] the cursor before those lines
 * @property {any[]} [conflicts]
 */

/**
 * The units a result is about, in the order it lists them.
 * @param {any} sc
 * @returns {Subject[]}
 */
function subjectsOf(sc) {
  if (card.drill && card.drill.unit) {
    const [lane, unit] = card.drill.unit.split(':');
    return [{ lane, unit }];
  }
  const data = (sc && sc.data) || {};
  if (Array.isArray(data.reasons)) {
    return [{ lane: data.lane, unit: data.unit, state: data.state, reasons: data.reasons }];
  }
  if (Array.isArray(data.failures) && data.failures.length) {
    return data.failures.map((f) => ({ lane: f.lane, unit: f.unit, exit: f.exit_code, reasons: f.reason ? [f.reason] : [], tail: f.log_tail }));
  }
  if (data.lane && Array.isArray(data.units) && data.units[0] && 'lines' in data.units[0]) {
    return data.units.map((group) => ({ lane: data.lane, unit: group.unit, lines: group.lines }));
  }
  const items = Array.isArray(data.items) ? data.items : [];
  if (items[0] && 'error_lines' in items[0]) {
    return items.map((e) => ({
      lane: e.lane,
      unit: e.unit,
      state: e.state,
      exit: e.exit_code,
      crashes: e.crashes,
      reasons: e.reason ? [e.reason] : [],
      tail: e.error_lines,
      conflicts: e.port_conflicts,
    }));
  }
  if (items[0] && 'seq' in items[0]) {
    // Log lines name their unit only; a lane the command named (`acme@2:api`) goes with it.
    const named = ((sc && sc.args) || []).map((arg) => /^([^\s:]+@\d+):/.exec(arg)).find(Boolean);
    const units = [...new Set(items.map((line) => line.unit))];
    return units.map((unit) => ({
      lane: named ? named[1] : undefined,
      unit,
      lines: items.filter((line) => line.unit === unit),
      next: units.length === 1 ? data.next_cursor : null,
    }));
  }
  return [];
}

/**
 * The unit's latest summary from the snapshot, when the poll has one.
 * @param {Subject} subject
 */
function liveUnit(subject) {
  const lanes = (card.snapshot && card.snapshot.status && card.snapshot.status.items) || [];
  const lane = lanes.find((l) => l.reference === subject.lane) || (lanes.length === 1 ? lanes[0] : null);
  return lane ? (lane.units || []).find((unit) => unit.name === subject.unit) : null;
}

/** The failure card. */
export function FailureView() {
  const subjects = subjectsOf(structured());
  const [index, setIndex] = useState(0);
  const back = card.drill && card.drill.unit
    ? html`<${Button} icon="back" label="back" iconOnly onClick=${() => update({ drill: card.drill && card.drill.lane ? { lane: card.drill.lane } : null })} />`
    : null;
  const tools = html`${back}<${FullscreenButton} />`;
  const subject = subjects[Math.min(index, subjects.length - 1)];
  if (!subject) {
    const sc = structured() || {};
    // A command that failed before it had a unit to show, such as an unknown lane.
    if (sc.exit_code || (sc.data && sc.data.error)) return html`<${TextView} />`;
    const command = sc.command || '';
    return html`<${Window} title=${html`<b>devlanes ${command}</b>`} label="failures" tools=${tools}>
      <p class="status-line"><${Glyph} state="ready" /> ${command === 'logs' ? 'no lines' : 'nothing failed'}</p>
    <//>`;
  }
  const ref = unitRef(subject.lane || '', subject.unit);
  return html`<${Window} title=${html`<b>${ref}</b>`} label=${`failure of ${ref}`} tools=${tools} foot=${html`<${PollStatus} />`}>
    ${subjects.length > 1 && html`<div class="tabs" role="tablist" aria-label="units">${subjects.map((s, i) => html`<button type="button" role="tab" class="tab" aria-selected=${i === index} onClick=${() => setIndex(i)}>${unitRef(s.lane || '', s.unit)}</button>`)}</div>`}
    <${Banner} subject=${subject} ref_=${ref} />
    <${LogViewer} key=${ref} subject=${subject} ref_=${ref} />
  <//>`;
}

/**
 * What failed and why: state, exit, when, who started it, the reasons and
 * where to look next.
 * @param {{subject: Subject, ref_: string}} props
 */
function Banner({ subject, ref_ }) {
  const unit = liveUnit(subject);
  const state = unit ? unitState(unit) : subject.state || (subject.exit != null || subject.tail ? 'failed' : '');
  const exit = unit ? unit.exit_code : subject.exit;
  const now = Date.now();
  const facts = [];
  if (exit != null && state !== 'ready') facts.push(`exit ${exit}`);
  if (unit && unit.finished_at_ms) facts.push(`${since(now - unit.finished_at_ms)} ago`);
  if (unit && unit.finished_at_ms && unit.started_at_ms) facts.push(`after ${since(unit.finished_at_ms - unit.started_at_ms)}`);
  if (unit && unit.actor) facts.push(`started by ${actorLabel(unit.actor)}`);
  if (subject.crashes) facts.push(`${subject.crashes} ${subject.crashes === 1 ? 'crash' : 'crashes'}`);
  const reasons = [...(subject.reasons || []), ...(unit && unit.reason && !(subject.reasons || []).length ? [unit.reason] : [])];
  const failed = state === 'failed';
  if (!state) return null;
  return html`<div class=${`banner${failed ? ' failure' : ''}`}>
    <p><${Glyph} state=${state} /> <b class=${failed ? 'tone-failure' : ''}>${state}</b>${facts.length > 0 && html` <span class="dim">· ${facts.join(' · ')}</span>`}</p>
    ${reasons.length > 0 && html`<ul>${reasons.map((reason) => html`<li><span class="dim">reason</span> ${reasonText(reason)}</li>`)}</ul>`}
    ${(subject.conflicts || []).map((c) => html`<p><span class="dim">port</span> ${c.port} is held by ${c.pid ? `pid ${c.pid}` : 'another process'}</p>`)}
    ${failed && html`<p><span class="dim">next</span> ${!readOnly() && 'restart it, or '}<code>devlanes logs ${ref_} --tail 200</code></p>`}
  </div>`;
}

/**
 * Fetches a page of one unit's logs through `app_logs`.
 * @param {string} ref
 * @param {object} args
 * @returns {Promise<{items?: any[], next_cursor?: string | null}>}
 */
async function fetchLogs(ref, args) {
  const result = await bridge.callTool('app_logs', { cwd: cwd(), units: [ref], ...args });
  return (result.structuredContent && result.structuredContent.logs) || {};
}

/**
 * The log viewer. Lines carry their cursor (`api#118`); a click picks one, and
 * "send to agent" hands the agent that cursor instead of the whole log.
 * @param {{subject: Subject, ref_: string}} props
 */
function LogViewer({ subject, ref_ }) {
  const fromResult = subject.lines || (subject.tail || []).map((text) => ({ unit: subject.unit, seq: null, text, level: null }));
  const [lines, setLines] = useState(/** @type {any[]} */ (fromResult));
  const [earlier, setEarlier] = useState(/** @type {string | null} */ (subject.next || null));
  const [grep, setGrep] = useState('');
  const [picked, setPicked] = useState(/** @type {number | null} */ (null));
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState('');
  const list = useRef(/** @type {HTMLElement | null} */ (null));
  const stick = useRef(true);
  const first = useRef(true);

  useEffect(() => {
    // A logs result already holds the latest page; anything else fetches it.
    if (first.current && subject.lines && !grep) {
      first.current = false;
      return undefined;
    }
    first.current = false;
    let gone = false;
    const timer = setTimeout(async () => {
      setLoading(true);
      try {
        const page = await fetchLogs(ref_, { tail: PAGE, grep: grep || undefined });
        if (gone) return;
        setLines(page.items || []);
        setEarlier(page.next_cursor || null);
        setPicked(null);
        setError('');
        stick.current = true;
      } catch (e) {
        if (!gone) setError(/** @type {Error} */ (e).message);
      } finally {
        if (!gone) setLoading(false);
      }
    }, grep ? 300 : 0);
    return () => {
      gone = true;
      clearTimeout(timer);
    };
  }, [ref_, grep]);

  // Each snapshot tick also reads the lines after the last one, unless a grep
  // narrows the view or the lines came without cursors.
  useEffect(() => {
    const last = lines[lines.length - 1];
    if (grep || !last || last.seq == null || card.snapshotSeq === 0) return;
    let gone = false;
    fetchLogs(ref_, { cursor: `${last.unit}#${last.seq}` })
      .then((page) => {
        if (gone || !(page.items || []).length) return;
        setLines((old) => [...old, ...(page.items || [])].slice(-MAX_LINES));
      })
      .catch(() => {});
    return () => {
      gone = true;
    };
  }, [card.snapshotSeq]);

  useLayoutEffect(() => {
    const el = list.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, [lines]);

  const loadEarlier = async () => {
    if (!earlier) return;
    setLoading(true);
    try {
      const page = await fetchLogs(ref_, { tail: PAGE, cursor: earlier, grep: grep || undefined });
      const items = page.items || [];
      stick.current = false;
      setLines((old) => [...items, ...old]);
      setPicked((p) => (p == null ? p : p + items.length));
      setEarlier(page.next_cursor || null);
    } catch (e) {
      setError(/** @type {Error} */ (e).message);
    } finally {
      setLoading(false);
    }
  };

  const onScroll = () => {
    const el = list.current;
    if (el) stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 8;
  };

  const line = picked == null ? null : lines[picked];
  const cursor = line && line.seq != null ? `${line.unit}#${line.seq}` : null;
  const send = () => {
    if (!line) return;
    ask(cursor
      ? `Look at ${ref_} around ${cursor}: devlanes logs ${ref_} --around ${cursor} -C 10`
      : `Look at this line from ${ref_}: ${line.text}`);
  };
  const busy = card.busy[ref_];

  return html`<div class="logs">
    <div class="toolbar">
      <label class="grep"><${Icon} name="grep" /><span class="sr-only">grep the logs of ${ref_}</span><input type="search" placeholder="grep" value=${grep} onInput=${(e) => setGrep(e.currentTarget.value)} spellcheck="false" autocomplete="off" /></label>
      <${Button} label="load earlier" disabled=${!earlier || loading} onClick=${loadEarlier} />
    </div>
    <ol class="logview" ref=${list} onScroll=${onScroll} aria-label=${`logs of ${ref_}`} aria-busy=${loading}>
      ${lines.length === 0 && html`<li class="dim">${loading ? 'loading…' : grep ? 'no line matches' : 'no lines'}</li>`}
      ${lines.map((l, i) => html`<li key=${l.seq == null ? i : `${l.unit}#${l.seq}`}><button type="button" class=${`line${i === picked ? ' picked' : ''}${l.level === 'error' ? ' lvl-error' : l.level === 'warn' ? ' lvl-warn' : ''}`} aria-pressed=${i === picked} onClick=${() => setPicked(i === picked ? null : i)}>${l.seq != null && html`<span class="cur">${l.unit}#${l.seq}</span>`}<span class="txt">${l.text}</span></button></li>`)}
    </ol>
    ${error && html`<p class="note tone-failure" role="status">${error}</p>`}
    <${Note} note=${card.note} />
    <div class="row">
      <span class="dim">${line ? html`picked <span class="url">${cursor || `line ${picked == null ? '' : picked + 1}`}</span>` : 'click a line to pick it'}</span>
      <span class="actions">
        ${!readOnly() && html`<${Button} icon="restart" label=${`restart ${subject.unit}`} disabled=${!!busy} onClick=${() => act('restart', ref_, `restarted ${ref_}`)} />`}
        <${Button} icon="ask" label=${cursor ? `send ${cursor} to agent` : 'send line to agent'} primary disabled=${!line} onClick=${send} />
      </span>
    </div>
  </div>`;
}
