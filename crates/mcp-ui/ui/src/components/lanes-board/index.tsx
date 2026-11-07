// The lanes board: one row per lane, grouped by project, with the lanes of linked boxes
// that work here as read-only rows. Inline it has at most two buttons; fullscreen adds
// the CPU and a button each to open, stop or start and ask about a lane.

import { ask, display } from '../../store/actions';
import { canFullscreen, canHandOff, card, isFullscreen, structured } from '../../store/card';
import type { StatusPage } from '../../types/proto';
import { atIndex } from '../../utils/format';
import { boxLanes, laneState } from '../../utils/lanes';
import { Button } from '../button';
import { Actions, Card, Head, Line, Pill } from '../card';
import { BoxRow, boxWarning, usable } from './box-row';
import { failedQuestion, LaneRow } from './lane-row';

/** The lanes board. */
export function LanesBoard() {
  const status: Partial<StatusPage> = card.snapshot?.status || structured()?.data || {};
  const lanes = status.items || [];
  const hosts = status.hosts || [];
  const allBoxed = boxLanes(card.snapshot?.hosts);
  const boxed = allBoxed.filter(usable);
  const warning = boxWarning(allBoxed, hosts);
  const manage = isFullscreen();
  const projects = [...new Set([...lanes, ...boxed].map((lane) => lane.project))];
  const failed = lanes.filter((lane) => laneState(lane) === 'failed');
  const [first] = failed;
  const count = `${lanes.length} ${lanes.length === 1 ? 'lane' : 'lanes'}${boxed.length ? ` · ${boxed.length} on boxes` : ''}`;
  return (
    <Card label="lanes">
      <Head
        title={projects.length === 1 ? projects[0] : 'Lanes'}
        sub={lanes.length || boxed.length ? count : undefined}
        pill={failed.length > 0 && <Pill tone="danger">{failed.length} failed</Pill>}
      />
      {!lanes.length && !boxed.length && <Line>No lanes yet.</Line>}
      {projects.map((project) => (
        <section key={project} aria-label={project}>
          {projects.length > 1 && <p class="group">{project}</p>}
          <ul class="rows">
            {lanes
              .filter((lane) => lane.project === project)
              .map((lane) => (
                <LaneRow key={lane.reference} lane={lane} manage={manage} />
              ))}
            {boxed
              .filter((lane) => lane.project === project)
              .map((lane) => (
                <BoxRow key={lane.reference} lane={lane} manage={manage} />
              ))}
          </ul>
        </section>
      ))}
      {warning && <Line tone="warn">{warning}</Line>}
      {!manage && (
        <Actions>
          {canFullscreen() && (lanes.length > 0 || boxed.length > 0) && (
            <Button key="manage" label="Manage lanes" onClick={() => display('fullscreen')} />
          )}
          {first && canHandOff() && (
            <Button
              key="ask"
              label={`Ask agent about ${atIndex(first.reference)}`}
              kind="primary"
              onClick={() => ask(failedQuestion(first))}
            />
          )}
        </Actions>
      )}
      {manage && <Actions />}
    </Card>
  );
}
