import { card, currentView, resultLane, structured } from '../../store/card';
import { Card, Line } from '../card';
import { CleanPlanner } from '../clean-planner';
import { FailureCard } from '../failure-card';
import { LaneCard } from '../lane-card';
import { LanesBoard } from '../lanes-board';
import { PairingCard } from '../pairing-card';
import { TextCard } from '../text-card';

/** An older card once a newer one for its lane is in the chat: one line, no buttons. */
function Superseded() {
  return (
    <section class="card" aria-label="older card">
      <p class="meta">
        <span class="t">{resultLane(structured())}</span> · newer card below
      </p>
    </section>
  );
}

/** The card for the current state. */
export function App() {
  if (card.failed) {
    return (
      <Card label="devlanes">
        <Line tone="warn">{card.failed}</Line>
      </Card>
    );
  }
  if (!card.connected) return null;
  if (card.superseded) return <Superseded />;
  switch (currentView()) {
    case 'lane':
      return <LaneCard />;
    case 'board':
      return <LanesBoard />;
    case 'failure':
      return <FailureCard />;
    case 'clean':
      return <CleanPlanner />;
    case 'pair':
      return <PairingCard />;
    // The host shows its own progress while a call runs and the result's text after
    // it, so these render nothing and report a height of 0.
    case 'pending':
    case null:
      return null;
    default:
      return <TextCard />;
  }
}
