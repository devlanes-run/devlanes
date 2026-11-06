// @ts-check
// The lane card: start phases while a lane comes up, then one row per service
// with its URL, CPU and buttons. Shown for start, restart, reload and the
// status of one lane, and for a lane opened from the board.
import { html } from '../vendor/standalone.module.js';
import { Button, Glyph, Note, Pane, Spark, Spinner, Window } from './ui.js';
import {
  FullscreenButton,
  PollStatus,
  act,
  ask,
  card,
  findLane,
  openUrl,
  resultLane,
  structured,
  update,
} from './store.js';
import {
  commandLine,
  cpuHistory,
  isBusy,
  isQuiet,
  laneState,
  laneUrl,
  laneWords,
  phasesOf,
  portOf,
  servicesOf,
  shortUrl,
  unitRef,
  unitState,
} from './util.js';

/** The lane card. */
export function LaneView() {
  const sc = structured();
  const ref = (card.drill && card.drill.lane) || resultLane(sc);
  const lane = findLane(ref);
  const back = card.drill && html`<${Button} icon="back" label="back to every lane" iconOnly onClick=${() => update({ drill: null })} />`;
  const tools = html`${back}<${FullscreenButton} />`;
  if (!lane) return html`<${PendingView} tools=${tools} />`;

  const now = Date.now();
  const state = laneState(lane);
  const phases = phasesOf(lane);
  const showPhases = !card.result || isBusy(state) || phases.some((phase) => phase.state === 'failed');
  const url = laneUrl(lane);
  const up = !['idle', 'stopped', 'no-config', 'unassigned'].includes(state);
  const title = html`<b>${lane.reference}</b>${lane.branch && html` <span class="dim">${lane.name || lane.branch}</span>`}`;
  const busy = card.busy[lane.reference];
  const failed = (lane.units || []).filter((unit) => unit.state === 'failed').map((unit) => unit.name);
  const question = failed.length
    ? `${lane.reference} (${lane.branch || 'no branch'}): ${failed.join(', ')} failed. Find out why and tell me before changing anything.`
    : `Check ${lane.reference} (${lane.branch || 'no branch'}) and tell me if anything needs attention.`;

  const foot = html`<${PollStatus} />
    <span class="actions">
      <${Button} icon="ask" label="ask agent" onClick=${() => ask(question)} />
      ${lane.config_changed && html`<${Button} icon="restart" label="reload" disabled=${!!busy} onClick=${() => act('reload', lane.reference, `reloaded ${lane.reference}`)} />`}
      ${up
        ? html`<${Button} icon="stop" label="stop lane" disabled=${!!busy} onClick=${() => act('stop', lane.reference, `stopped ${lane.reference}`)} />`
        : html`<${Button} icon="start" label="start lane" disabled=${!!busy} onClick=${() => act('start', lane.reference, `started ${lane.reference}`)} />`}
      ${url && html`<${Button} icon="open" label="open" primary onClick=${() => openUrl(url)} />`}
    </span>`;

  return html`<${Window} title=${title} label=${`lane ${lane.reference}`} tools=${tools} foot=${foot}>
    <p class="status-line"><${Glyph} state=${busy ? 'stopping' : state === 'running' ? 'ready' : state} /> <span class=${`tone-${state === 'failed' ? 'failure' : 'plain'}`}>${busy ? `${busy}…` : laneWords(lane, now)}</span>${lane.domain && html` <span class="dim">· ${lane.domain}</span>`}</p>
    <${Warnings} lane=${lane} />
    ${showPhases && phases.length > 0 && html`<${Phases} phases=${phases} />`}
    <${Units} lane=${lane} />
    <${Note} note=${card.note} />
  <//>`;
}

/**
 * A card for a call that has not answered yet, or answered without a lane:
 * the command line and a spinner.
 * @param {{tools: any}} props
 */
export function PendingView({ tools }) {
  const args = (card.input && card.input.args) || [];
  return html`<${Window} title=${html`<b>devlanes</b>`} label="devlanes command" tools=${tools} foot=${html`<${PollStatus} />`}>
    <p class="cmd">$ ${commandLine(args)}</p>
    ${card.cancelled
      ? html`<p class="tone-attention">! cancelled: ${card.cancelled}. Services the call started keep running.</p>`
      : html`<p class="status-line"><${Spinner} /> ${card.input ? 'running…' : 'waiting for the tool call…'}</p>`}
  <//>`;
}

/**
 * Conditions that need a look without stopping the lane.
 * @param {{lane: any}} props
 */
function Warnings({ lane }) {
  const lines = [];
  if (lane.needs_env) lines.push('the lane lost its environment; a start from a shell brings it back');
  if (lane.config_changed) lines.push('devlanes.yaml changed since the lane started; reload applies it');
  for (const warning of lane.warnings || []) lines.push((warning.data && warning.data.message) || warning.code);
  if (card.cancelled) lines.push(`cancelled: ${card.cancelled}; services keep starting`);
  const power = lane.power && html`<li class="dim">${lane.power.level} while idle; its processes keep their state and the next request or connection thaws it</li>`;
  if (!lines.length && !power) return null;
  return html`<ul class="warnings">${lines.map((line) => html`<li class="tone-attention">! ${line}</li>`)}${power}</ul>`;
}

/**
 * The start phases in a notched pane.
 * @param {{phases: {name: string, state: string, detail: string}[]}} props
 */
function Phases({ phases }) {
  return html`<${Pane} title="start" cls="phases">
    <ul>${phases.map((phase) => html`<li><${Glyph} state=${phase.state} /><span class="ph-name">${phase.name}</span><span class="ph-detail">${phase.detail}</span></li>`)}</ul>
  <//>`;
}

/**
 * One row per service: glyph, name (opens its logs), state when it needs a
 * word, port, URL, CPU and buttons.
 * @param {{lane: any}} props
 */
function Units({ lane }) {
  const units = servicesOf(lane);
  if (!units.length) return html`<p class="dim">no services; ${lane.state === 'idle' ? 'start the lane to run them' : 'this lane runs none'}</p>`;
  const top = card.snapshot && card.snapshot.top;
  return html`<table class="units">
    <caption class="sr-only">services of ${lane.reference}</caption>
    <tbody>${units.map((unit) => {
      const ref = unitRef(lane.reference, unit.name);
      const state = card.busy[ref] ? 'starting' : unitState(unit);
      const port = portOf(unit);
      const live = unit.state === 'ready' || isBusy(unit.state);
      return html`<tr key=${unit.name}>
        <td class="g"><${Glyph} state=${state} /></td>
        <th scope="row"><button type="button" class="link name" title=${`logs of ${ref}`} onClick=${() => update({ drill: { ...card.drill, unit: ref } })}>${unit.display_name || unit.name}</button>${!isQuiet(state) && html` <span class=${`word tone-${state === 'failed' ? 'failure' : 'plain'}`}>${card.busy[ref] ? `${card.busy[ref]}…` : state}${unit.exit_code != null && state !== 'done' ? ` · exit ${unit.exit_code}` : ''}</span>`}</th>
        <td class="port dim">${port && `:${port}`}</td>
        <td class="url">${unit.public_url && unit.state === 'ready' && html`<button type="button" class="link url" onClick=${() => openUrl(unit.public_url)}>${shortUrl(unit.public_url)}</button>`}</td>
        <td class="cpu"><${Spark} values=${cpuHistory(top, lane.reference, unit.name)} label=${`${unit.name} cpu`} /></td>
        <td class="act">${live
          ? html`<${Button} icon="restart" label=${`restart ${unit.name}`} iconOnly disabled=${!!card.busy[ref]} onClick=${() => act('restart', ref, `restarted ${ref}`)} /><${Button} icon="stop" label=${`stop ${unit.name}`} iconOnly disabled=${!!card.busy[ref]} onClick=${() => act('stop', ref, `stopped ${ref}`)} />`
          : html`<${Button} icon=${state === 'failed' ? 'restart' : 'start'} label=${`${state === 'failed' ? 'restart' : 'start'} ${unit.name}`} iconOnly disabled=${!!card.busy[ref]} onClick=${() => act(state === 'failed' ? 'restart' : 'start', ref, `${state === 'failed' ? 'restarted' : 'started'} ${ref}`)} />`}</td>
      </tr>`;
    })}</tbody>
  </table>`;
}
