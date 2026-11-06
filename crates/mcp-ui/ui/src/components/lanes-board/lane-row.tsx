import { act, ask, openUrl } from '../../store/actions';
import { canHandOff, card, daemonStopped, readOnly } from '../../store/card';
import type { LaneSummary } from '../../types/proto';
import { atIndex, shortUrl } from '../../utils/format';
import {
  actorsOf,
  cpuHistory,
  domainService,
  laneState,
  laneWords,
  servicesOf
} from '../../utils/lanes';
import { powerHint, wordClass } from '../../utils/state';
import { IconButton, LinkButton } from '../button';
import { Hint, Shape } from '../card';
import { Spark } from '../spark';

/** The question "Ask agent about @2" sends for a lane with failed services. */
export function failedQuestion(lane: LaneSummary): string {
  const failed = lane.units.filter((unit) => unit.state === 'failed').map((unit) => unit.name);
  const branch = lane.name || lane.branch;
  return `${lane.reference}${branch ? ` (${branch})` : ''}: ${failed.join(', ') || 'a service'} failed. Why?`;
}

/** The URL a lane opens at: its domain's service once that is ready. */
export function openTarget(lane: LaneSummary): string | undefined {
  const web = domainService(lane);
  return web?.state === 'ready' && web.public_url ? web.public_url : undefined;
}

/**
 * One lane of this machine: shape, `@2`, branch, who started it from 520 px, the lane's
 * URL and its state with the ready count. `manage` adds the fullscreen columns: CPU and
 * a button each to open, stop or start, and ask about the lane.
 */
export function LaneRow({ lane, manage }: { lane: LaneSummary; manage: boolean }) {
  const busy = card.busy[lane.reference];
  const state = laneState(lane);
  const url = daemonStopped() ? undefined : openTarget(lane);
  const up = !['idle', 'stopped', 'no-config', 'unassigned'].includes(state);
  const actors = actorsOf(servicesOf(lane));
  const ref = lane.reference;
  const label = busy
    ? `${busy === 'stop' ? 'stopping' : 'starting'}…`
    : laneWords(lane, Date.now());
  return (
    <li class="row">
      <Shape state={busy ? 'starting' : state} />
      <span class="name num">{lane.index == null ? ref : atIndex(ref)}</span>
      <span class="grow">{lane.name || lane.branch || ''}</span>
      {actors.length > 0 && <span class="col meta w520">{actors.join(', ')}</span>}
      {!manage && url && (
        <LinkButton title={url} onClick={() => openUrl(url)}>
          {shortUrl(url)}
        </LinkButton>
      )}
      {manage && (
        <span class="w640">
          <Spark values={cpuHistory(card.snapshot?.top, ref)} label={`${ref} cpu`} />
        </span>
      )}
      <span class={`end ${wordClass(state)}`} title={busy ? undefined : powerHint(state)}>
        {label}
        {!busy && <Hint state={state} />}
      </span>
      {manage && (
        <span class="acts">
          <IconButton
            icon="open"
            label={`Open ${ref}`}
            onClick={url ? () => openUrl(url) : undefined}
          />
          <IconButton
            icon={up ? 'stop' : 'start'}
            label={`${up ? 'Stop' : 'Start'} ${ref}`}
            disabled={!!busy}
            onClick={
              readOnly()
                ? undefined
                : () =>
                    up ? act('stop', ref, `stopped ${ref}`) : act('start', ref, `started ${ref}`)
            }
          />
          <IconButton
            icon="ask"
            label={`Ask agent about ${ref}`}
            onClick={
              canHandOff()
                ? () =>
                    ask(state === 'failed' ? failedQuestion(lane) : `What is going on in ${ref}?`)
                : undefined
            }
          />
        </span>
      )}
    </li>
  );
}
