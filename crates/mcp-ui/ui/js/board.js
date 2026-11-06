// @ts-check
// The lanes board: every lane of the status as the website's lane table, who
// started it, its state and buttons, then the boxes this machine mirrors, which
// are read-only here.
import { html } from '../vendor/standalone.module.js';
import { Actors, Button, Glyph, LaneMark, Note, Spark, Window } from './ui.js';
import { FullscreenButton, PollStatus, act, ask, card, openUrl, structured, update } from './store.js';
import { activeServices, actorsOf, boxLanes, cpuHistory, laneState, laneUrl, laneWords, servicesOf, toneOf } from './util.js';

/** The lanes board. */
export function BoardView() {
  const sc = structured();
  const status = (card.snapshot && card.snapshot.status) || (sc && sc.data) || {};
  const lanes = status.items || [];
  const hosts = status.hosts || [];
  // A box lane joins the rows of this machine's project of the same name.
  const boxed = boxLanes(card.snapshot && card.snapshot.hosts);
  const projects = [...new Set([...lanes, ...boxed].map((lane) => lane.project))];
  const count = `${lanes.length} ${lanes.length === 1 ? 'lane' : 'lanes'}${boxed.length ? ` · ${boxed.length} on boxes` : ''}`;
  const title = projects.length === 1
    ? html`<b>${projects[0]}</b> <span class="dim">· ${count}</span>`
    : html`<b>devlanes status</b> <span class="dim">· ${count}</span>`;
  return html`<${Window} title=${title} label="lanes" tools=${html`<${FullscreenButton} />`} foot=${html`<${PollStatus} />`}>
    ${lanes.length || boxed.length
      ? html`<div class="table-scroll" role="region" aria-label="lanes, scroll for every column" tabindex="0">
          <table class="lanes">
            <thead><tr><th scope="col">lane</th><th scope="col">branch</th><th scope="col">state</th><th scope="col" class="svc">services</th><th scope="col" class="by">by</th><th scope="col" class="cpu">cpu</th><th scope="col"><span class="sr-only">actions</span></th></tr></thead>
            ${projects.map((project) => html`<tbody>
              ${projects.length > 1 && html`<tr class="caption"><th colspan="7" scope="rowgroup">${project}</th></tr>`}
              ${lanes.filter((lane) => lane.project === project).map((lane) => html`<${Row} lane=${lane} key=${lane.reference} />`)}
              ${boxed.filter((lane) => lane.project === project).map((lane) => html`<${BoxRow} lane=${lane} key=${lane.reference} />`)}
            </tbody>`)}
          </table>
        </div>`
      : html`<p class="empty">no lanes yet · run <code>devlanes start</code> in a repository with a devlanes.yaml</p>`}
    ${hosts.length > 0 && html`<${Boxes} hosts=${hosts} />`}
    <${Note} note=${card.note} />
  <//>`;
}

/**
 * One lane of the table; the project's caption names the project.
 * @param {{lane: any}} props
 */
function Row({ lane }) {
  const now = Date.now();
  const state = card.busy[lane.reference] ? 'starting' : laneState(lane);
  const active = activeServices(lane);
  const ready = active.filter((unit) => unit.state === 'ready').length;
  const url = laneUrl(lane);
  const up = !['idle', 'stopped', 'no-config', 'unassigned'].includes(state);
  const busy = !!card.busy[lane.reference];
  const branch = lane.name || lane.branch || '–';
  const failed = (lane.units || []).filter((unit) => unit.state === 'failed').map((unit) => unit.name);
  const question = failed.length
    ? `${lane.reference} (${branch}): ${failed.join(', ')} failed. Find out why and tell me before changing anything.`
    : `What is the state of ${lane.reference} (${branch})?`;
  const top = card.snapshot && card.snapshot.top;
  return html`<tr class=${lane.index === 0 ? 'lane-zero' : ''}>
    <td class="lane"><${LaneMark} index=${lane.index} /><button type="button" class="link" title=${`show ${lane.reference}`} onClick=${() => update({ drill: { lane: lane.reference } })}>${lane.index == null ? lane.reference : `@${lane.index}`}</button></td>
    <th scope="row" class="branch" title=${branch}>${branch}</th>
    <td class="state"><${Glyph} state=${state === 'running' ? 'ready' : state} /> <span class=${state === 'failed' ? 'tone-failure' : toneOf(state) === 'quiet' ? 'dim' : ''}>${busy ? `${card.busy[lane.reference]}…` : laneWords(lane, now)}</span></td>
    <td class="svc dim">${active.length ? `${ready}/${active.length}` : '–'}</td>
    <td class="by"><${Actors} actors=${actorsOf(servicesOf(lane))} /></td>
    <td class="cpu"><${Spark} values=${cpuHistory(top, lane.reference)} label=${`${lane.reference} cpu`} /></td>
    <td class="act">
      ${url && html`<${Button} icon="open" label=${`open ${lane.reference}`} iconOnly onClick=${() => openUrl(url)} />`}
      ${up
        ? html`<${Button} icon="stop" label=${`stop ${lane.reference}`} iconOnly disabled=${busy} onClick=${() => act('stop', lane.reference, `stopped ${lane.reference}`)} />`
        : html`<${Button} icon="start" label=${`start ${lane.reference}`} iconOnly disabled=${busy} onClick=${() => act('start', lane.reference, `started ${lane.reference}`)} />`}
      <${Button} icon="ask" label=${`ask the agent about ${lane.reference}`} iconOnly cls="ask" onClick=${() => ask(question)} />
    </td>
  </tr>`;
}

/** Why a box's lanes show their last known state, after the TUI's box pane. */
const LINK_WORDS = {
  needs_auth: 'needs you to sign in again',
  needs_pairing: 'needs a new pairing code',
  connecting: 'is reconnecting',
  reconnecting: 'is reconnecting',
};

/**
 * A lane of a linked box: read-only, so its only button opens it, with the URL
 * that works on this machine. A lane this machine does not mirror, and every
 * lane of a box whose link is down, is dimmed with why. `remote ls` has no
 * branch for box lanes, so that cell says where to manage the lane.
 * @param {{lane: any}} props
 */
function BoxRow({ lane }) {
  const active = activeServices(lane);
  const ready = active.filter((unit) => unit.state === 'ready').length;
  const down = lane.link !== 'connected';
  const url = lane.skipped ? undefined : laneUrl(lane);
  const words = lane.skipped
    ? `not mirrored: ${lane.skipped.reason || 'its hostnames are served already'}`
    : `${laneWords(lane, Date.now())}${down ? ' · last known' : ''}`;
  const title = down
    ? `${lane.host} ${LINK_WORDS[lane.link] || `is ${String(lane.link).replace(/_/g, ' ')}`}, so this is its last known state`
    : `${lane.box} runs on ${lane.host}; manage it there`;
  return html`<tr class=${`box-lane${lane.skipped || down ? ' skipped' : ''}`} title=${title}>
    <td class="lane"><${LaneMark} index=${lane.index} />@${lane.index} <span class="box-name dim">@${lane.host}</span></td>
    <th scope="row" class="branch dim">manage on ${lane.host}</th>
    <td class="state"><${Glyph} state=${lane.state === 'running' ? 'ready' : lane.state} /> <span class=${lane.skipped || toneOf(lane.state) === 'quiet' ? 'dim' : lane.state === 'failed' ? 'tone-failure' : ''}>${words}</span></td>
    <td class="svc dim">${active.length ? `${ready}/${active.length}` : '–'}</td>
    <td class="by"></td>
    <td class="cpu"></td>
    <td class="act">${url && html`<${Button} icon="open" label=${`open ${lane.box} on ${lane.host}`} iconOnly onClick=${() => openUrl(url)} />`}</td>
  </tr>`;
}

/**
 * The boxes this machine is linked to. Their lanes run on the box, so the card
 * shows them and offers no buttons.
 * @param {{hosts: any[]}} props
 */
function Boxes({ hosts }) {
  return html`<section class="boxes" aria-label="boxes">
    <h2 class="label">boxes <span class="dim">· read-only here, manage their lanes on the box</span></h2>
    <ul>${hosts.map((host) => html`<li>
      <${Glyph} state=${host.state} /> <b>${host.host}</b> <span class="dim">${String(host.state || '').replace(/_/g, ' ')}</span>
      ${host.fix && html` <code>${host.fix}</code>`}
      ${host.error && html` <span class="dim">${host.error}</span>`}
    </li>`)}</ul>
  </section>`;
}
