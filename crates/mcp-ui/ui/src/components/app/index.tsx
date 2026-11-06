import { card, currentView, daemonStopped } from '../../store/card';
import { CleanPlanner } from '../clean-planner';
import { FailureCard } from '../failure-card';
import { FullscreenButton } from '../fullscreen-button';
import { LaneCard } from '../lane-card';
import { LanesBoard } from '../lanes-board';
import { PairingCard } from '../pairing-card';
import { PendingCard } from '../pending-card';
import { StoppedCard } from '../stopped-card';
import { TextCard } from '../text-card';
import { Window } from '../window';

/** The card for the current state. */
export function App() {
  if (card.failed) {
    return (
      <Window title={<b>devlanes</b>} label="devlanes">
        <p class="tone-attention">! {card.failed}</p>
        <p class="dim">this card runs inside an MCP Apps host, which answers its ui/initialize</p>
      </Window>
    );
  }
  if (!card.connected) return <p class="boot">connecting to the host…</p>;
  const view = currentView();
  // Only the views that poll the daemon have nothing to show without one.
  if (
    daemonStopped() &&
    (card.drill || view === 'lane' || view === 'board' || view === 'failure')
  ) {
    return <StoppedCard />;
  }
  if (card.drill?.unit) return <FailureCard />;
  switch (view) {
    case 'lane':
      return <LaneCard />;
    case 'board':
      return card.drill?.lane ? <LaneCard /> : <LanesBoard />;
    case 'failure':
      return <FailureCard />;
    case 'clean':
      return <CleanPlanner />;
    case 'pair':
      return <PairingCard />;
    case 'pending':
      return <PendingCard tools={<FullscreenButton />} />;
    // The host shows the result's text already, and an empty card reports height 0.
    case null:
      return null;
    default:
      return <TextCard />;
  }
}
