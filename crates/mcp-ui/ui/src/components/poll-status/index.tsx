import { card, daemonStopped } from '../../store/card';
import { every, pollNow } from '../../store/poll';
import { Button } from '../button';

/** Whether the card polls now, and a way back when it stopped. */
export function PollStatus() {
  switch (card.poll) {
    case 'live':
      if (card.pollError) {
        return <span class="poll tone-attention">retrying: {card.pollError}</span>;
      }
      if (daemonStopped()) return <span class="poll">checking every {every() / 1000}s</span>;
      return (
        <span class="poll">
          <span class="tone-ok" aria-hidden="true">
            ●
          </span>{' '}
          live · every {every() / 1000}s
        </span>
      );
    case 'paused':
      return <span class="poll">paused while hidden</span>;
    case 'idle':
      return (
        <span class="poll">
          stopped after 15m idle <Button label="resume" onClick={pollNow} />
        </span>
      );
    case 'error':
      return (
        <span class="poll tone-failure">
          can't reach devlanes: {card.pollError} <Button label="retry" onClick={pollNow} />
        </span>
      );
    default:
      return <span class="poll">{card.pollError && `retrying: ${card.pollError}`}</span>;
  }
}
