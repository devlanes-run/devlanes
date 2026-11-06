// The lanes board: every lane of the status as the website's lane table, who
// started it, its state and buttons, then the boxes this machine mirrors, which
// are read-only here.

import { card, structured } from '../../store/card';
import type { StatusPage } from '../../types/proto';
import { boxLanes } from '../../utils/lanes';
import { FullscreenButton } from '../fullscreen-button';
import { Note } from '../note';
import { PollStatus } from '../poll-status';
import { Window } from '../window';
import { BoxRow } from './box-row';
import { Boxes } from './boxes';
import { LaneRow } from './lane-row';

/** The lanes board. */
export function LanesBoard() {
  const status: Partial<StatusPage> = card.snapshot?.status || structured()?.data || {};
  const lanes = status.items || [];
  const hosts = status.hosts || [];
  // A box lane joins the rows of this machine's project of the same name.
  const boxed = boxLanes(card.snapshot?.hosts);
  const projects = [...new Set([...lanes, ...boxed].map((lane) => lane.project))];
  const count = `${lanes.length} ${lanes.length === 1 ? 'lane' : 'lanes'}${boxed.length ? ` · ${boxed.length} on boxes` : ''}`;
  const title =
    projects.length === 1 ? (
      <>
        <b>{projects[0]}</b> <span class="dim">· {count}</span>
      </>
    ) : (
      <>
        <b>devlanes status</b> <span class="dim">· {count}</span>
      </>
    );
  return (
    <Window title={title} label="lanes" tools={<FullscreenButton />} foot={<PollStatus />}>
      {lanes.length || boxed.length ? (
        // A labelled section is a region; it takes focus so a keyboard can scroll it.
        <section class="table-scroll" aria-label="lanes, scroll for every column" tabindex={0}>
          <table class="lanes">
            <thead>
              <tr>
                <th scope="col">lane</th>
                <th scope="col">branch</th>
                <th scope="col">state</th>
                <th scope="col" class="svc">
                  services
                </th>
                <th scope="col" class="by">
                  by
                </th>
                <th scope="col" class="cpu">
                  cpu
                </th>
                <th scope="col">
                  <span class="sr-only">actions</span>
                </th>
              </tr>
            </thead>
            {projects.map((project) => (
              <tbody key={project}>
                {projects.length > 1 && (
                  <tr class="caption">
                    <th colSpan={7} scope="rowgroup">
                      {project}
                    </th>
                  </tr>
                )}
                {lanes
                  .filter((lane) => lane.project === project)
                  .map((lane) => (
                    <LaneRow lane={lane} key={lane.reference} />
                  ))}
                {boxed
                  .filter((lane) => lane.project === project)
                  .map((lane) => (
                    <BoxRow lane={lane} key={lane.reference} />
                  ))}
              </tbody>
            ))}
          </table>
        </section>
      ) : (
        <p class="empty">
          no lanes yet · run <code>devlanes start</code> in a repository with a devlanes.yaml
        </p>
      )}
      {hosts.length > 0 && <Boxes hosts={hosts} />}
      <Note note={card.note} />
    </Window>
  );
}
