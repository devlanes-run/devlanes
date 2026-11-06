// The lane card: start phases while a lane comes up, then one row per service
// with its URL, CPU and buttons. Shown for start, restart, reload and the
// status of one lane, and for a lane opened from the board.

import { act, ask, openUrl } from '../../store/actions';
import { card, findLane, readOnly, resultLane, structured, update } from '../../store/card';
import { laneState, laneUrl, laneWords, phasesOf } from '../../utils/lanes';
import { isBusy } from '../../utils/state';
import { Button } from '../button';
import { FullscreenButton } from '../fullscreen-button';
import { Glyph } from '../glyph';
import { Note } from '../note';
import { PendingCard } from '../pending-card';
import { PollStatus } from '../poll-status';
import { TextCard } from '../text-card';
import { Window } from '../window';
import { Phases } from './phases';
import { Units } from './units';
import { Warnings } from './warnings';

/** The lane card. */
export function LaneCard() {
  const ref = card.drill?.lane || resultLane(structured());
  const lane = findLane(ref);
  const back = card.drill && (
    <Button
      icon="back"
      label="back to every lane"
      iconOnly
      onClick={() => update({ drill: null })}
    />
  );
  const tools = (
    <>
      {back}
      <FullscreenButton />
    </>
  );
  if (!lane) return card.result ? <TextCard /> : <PendingCard tools={tools} />;

  const now = Date.now();
  const state = laneState(lane);
  const phases = phasesOf(lane);
  // Phases matter while the lane comes up or after one failed; a ready lane shows its services.
  const showPhases =
    !card.result || isBusy(state) || phases.some((phase) => phase.state === 'failed');
  const url = laneUrl(lane);
  const up = !['idle', 'stopped', 'no-config', 'unassigned'].includes(state);
  const title = (
    <>
      <b>{lane.reference}</b>
      {lane.branch && (
        <>
          {' '}
          <span class="dim">{lane.name || lane.branch}</span>
        </>
      )}
    </>
  );
  const busy = card.busy[lane.reference];
  const failed = (lane.units || [])
    .filter((unit) => unit.state === 'failed')
    .map((unit) => unit.name);
  const question = failed.length
    ? `${lane.reference} (${lane.branch || 'no branch'}): ${failed.join(', ')} failed. Find out why and tell me before changing anything.`
    : `Check ${lane.reference} (${lane.branch || 'no branch'}) and tell me if anything needs attention.`;

  const ro = readOnly();
  const foot = (
    <>
      <PollStatus />
      <span class="actions">
        <Button icon="ask" label="ask agent" onClick={() => ask(question)} />
        {!ro && lane.config_changed && (
          <Button
            icon="restart"
            label="reload"
            disabled={!!busy}
            onClick={() => act('reload', lane.reference, `reloaded ${lane.reference}`)}
          />
        )}
        {ro ? null : up ? (
          <Button
            icon="stop"
            label="stop lane"
            disabled={!!busy}
            onClick={() => act('stop', lane.reference, `stopped ${lane.reference}`)}
          />
        ) : (
          <Button
            icon="start"
            label="start lane"
            disabled={!!busy}
            onClick={() => act('start', lane.reference, `started ${lane.reference}`)}
          />
        )}
        {url && <Button icon="open" label="open" primary onClick={() => openUrl(url)} />}
      </span>
    </>
  );

  return (
    <Window title={title} label={`lane ${lane.reference}`} tools={tools} foot={foot}>
      <p class="status-line">
        <Glyph state={busy ? 'stopping' : state === 'running' ? 'ready' : state} />{' '}
        <span class={`tone-${state === 'failed' ? 'failure' : 'plain'}`}>
          {busy ? `${busy}…` : laneWords(lane, now)}
        </span>
        {lane.domain && (
          <>
            {' '}
            <span class="dim">· {lane.domain}</span>
          </>
        )}
      </p>
      <Warnings lane={lane} />
      {showPhases && phases.length > 0 && <Phases phases={phases} />}
      <Units lane={lane} />
      <Note note={card.note} />
    </Window>
  );
}
