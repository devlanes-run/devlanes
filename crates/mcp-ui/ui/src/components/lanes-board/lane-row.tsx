import { act, ask, openUrl } from '../../store/actions';
import { card, readOnly, update } from '../../store/card';
import type { LaneSummary } from '../../types/proto';
import {
  activeServices,
  actorsOf,
  cpuHistory,
  laneState,
  laneUrl,
  laneWords,
  servicesOf
} from '../../utils/lanes';
import { toneOf } from '../../utils/state';
import { Button } from '../button';
import { Actors, LaneMark } from '../chips';
import { Glyph } from '../glyph';
import { Spark } from '../spark';

/** One lane of this machine in the board; the project's caption names the project. */
export function LaneRow({ lane }: { lane: LaneSummary }) {
  const now = Date.now();
  const busy = card.busy[lane.reference];
  const state = busy ? 'starting' : laneState(lane);
  const active = activeServices(lane);
  const ready = active.filter((unit) => unit.state === 'ready').length;
  const url = laneUrl(lane);
  const up = !['idle', 'stopped', 'no-config', 'unassigned'].includes(state);
  const branch = lane.name || lane.branch || '–';
  const failed = (lane.units || [])
    .filter((unit) => unit.state === 'failed')
    .map((unit) => unit.name);
  const question = failed.length
    ? `${lane.reference} (${branch}): ${failed.join(', ')} failed. Find out why and tell me before changing anything.`
    : `What is the state of ${lane.reference} (${branch})?`;
  const words = state === 'failed' ? 'tone-failure' : toneOf(state) === 'quiet' ? 'dim' : '';
  return (
    <tr class={lane.index === 0 ? 'lane-zero' : ''}>
      <td class="lane">
        <LaneMark index={lane.index} />
        <button
          type="button"
          class="link"
          title={`show ${lane.reference}`}
          onClick={() => update({ drill: { lane: lane.reference } })}
        >
          {lane.index == null ? lane.reference : `@${lane.index}`}
        </button>
      </td>
      <th scope="row" class="branch" title={branch}>
        {branch}
      </th>
      <td class="state">
        <Glyph state={state === 'running' ? 'ready' : state} />{' '}
        <span class={words}>{busy ? `${busy}…` : laneWords(lane, now)}</span>
      </td>
      <td class="svc dim">{active.length ? `${ready}/${active.length}` : '–'}</td>
      <td class="by">
        <Actors actors={actorsOf(servicesOf(lane))} />
      </td>
      <td class="cpu">
        <Spark
          values={cpuHistory(card.snapshot?.top, lane.reference)}
          label={`${lane.reference} cpu`}
        />
      </td>
      <td class="act">
        {url && (
          <Button
            icon="open"
            label={`open ${lane.reference}`}
            iconOnly
            onClick={() => openUrl(url)}
          />
        )}
        {readOnly() ? null : up ? (
          <Button
            icon="stop"
            label={`stop ${lane.reference}`}
            iconOnly
            disabled={!!busy}
            onClick={() => act('stop', lane.reference, `stopped ${lane.reference}`)}
          />
        ) : (
          <Button
            icon="start"
            label={`start ${lane.reference}`}
            iconOnly
            disabled={!!busy}
            onClick={() => act('start', lane.reference, `started ${lane.reference}`)}
          />
        )}
        <Button
          icon="ask"
          label={`ask the agent about ${lane.reference}`}
          iconOnly
          cls="ask"
          onClick={() => ask(question)}
        />
      </td>
    </tr>
  );
}
