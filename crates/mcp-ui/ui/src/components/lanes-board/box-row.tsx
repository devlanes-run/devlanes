import { openUrl } from '../../store/actions';
import type { BoxLane } from '../../types/remote';
import { activeServices, laneUrl, laneWords } from '../../utils/lanes';
import { toneOf } from '../../utils/state';
import { Button } from '../button';
import { LaneMark } from '../chips';
import { Glyph } from '../glyph';

/** Why a box's lanes show their last known state, after the TUI's box pane. */
const LINK_WORDS: Record<string, string> = {
  needs_auth: 'needs you to sign in again',
  needs_pairing: 'needs a new pairing code',
  connecting: 'is reconnecting',
  reconnecting: 'is reconnecting'
};

/**
 * A lane of a linked box: read-only, so its only button opens it, with the URL
 * that works on this machine. A lane this machine does not mirror, and every
 * lane of a box whose link is down, is dimmed with why and has no open button,
 * since its URL may not answer. `remote ls` has no branch for box lanes, so
 * that cell says where to manage the lane.
 */
export function BoxRow({ lane }: { lane: BoxLane }) {
  const active = activeServices(lane);
  const ready = active.filter((unit) => unit.state === 'ready').length;
  const down = lane.link !== 'connected';
  const url = lane.skipped || down ? undefined : laneUrl(lane);
  const words = lane.skipped
    ? `not mirrored: ${lane.skipped.reason || 'its hostnames are served already'}`
    : `${laneWords(lane, Date.now())}${down ? ' · last known' : ''}`;
  const title = down
    ? `${lane.host} ${LINK_WORDS[lane.link] || `is ${String(lane.link).replace(/_/g, ' ')}`}, so this is its last known state`
    : `${lane.box} runs on ${lane.host}; manage it there`;
  const tone =
    lane.skipped || toneOf(lane.state) === 'quiet'
      ? 'dim'
      : lane.state === 'failed'
        ? 'tone-failure'
        : '';
  return (
    <tr class={`box-lane${lane.skipped || down ? ' skipped' : ''}`} title={title}>
      <td class="lane">
        <LaneMark index={lane.index} />@{lane.index} <span class="box-name dim">@{lane.host}</span>
      </td>
      <th scope="row" class="branch dim">
        manage on {lane.host}
      </th>
      <td class="state">
        <Glyph state={lane.skipped ? 'idle' : lane.state === 'running' ? 'ready' : lane.state} />{' '}
        <span class={tone}>{words}</span>
      </td>
      <td class="svc dim">{active.length ? `${ready}/${active.length}` : '–'}</td>
      <td class="by" />
      <td class="cpu" />
      <td class="act">
        {url && (
          <Button
            icon="open"
            label={`open ${lane.box} on ${lane.host}`}
            iconOnly
            onClick={() => openUrl(url)}
          />
        )}
      </td>
    </tr>
  );
}
