import type { ComponentChildren } from 'preact';
import { card } from '../../store/card';
import { commandLine } from '../../utils/format';
import { Spinner } from '../glyph';
import { PollStatus } from '../poll-status';
import { Window } from '../window';

/**
 * A card for a call that has not answered yet, or answered without a lane:
 * the command line and a spinner.
 */
export function PendingCard({ tools }: { tools: ComponentChildren }) {
  const args = card.input?.args || [];
  return (
    <Window title={<b>devlanes</b>} label="devlanes command" tools={tools} foot={<PollStatus />}>
      <p class="cmd">$ {commandLine(args)}</p>
      {card.cancelled ? (
        <p class="tone-attention">
          ! cancelled: {card.cancelled}. Services the call started keep running.
        </p>
      ) : (
        <p class="status-line">
          <Spinner /> {card.input ? 'running…' : 'waiting for the tool call…'}
        </p>
      )}
    </Window>
  );
}
