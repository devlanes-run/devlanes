import { structured } from '../../store/card';
import { FullscreenButton } from '../fullscreen-button';
import { Glyph } from '../glyph';
import { PollStatus } from '../poll-status';
import { Window } from '../window';

/** A polling card while no daemon runs. The card never starts one, so it keeps checking. */
export function StoppedCard() {
  const sc = structured() || {};
  return (
    <Window
      title={<b>devlanes {sc.command || ''}</b>}
      label="devlanes"
      tools={<FullscreenButton />}
      foot={<PollStatus />}
    >
      <p class="status-line">
        <Glyph state="stopped" /> the daemon is not running
      </p>
      <p class="dim">the next devlanes command that needs it starts it</p>
    </Window>
  );
}
