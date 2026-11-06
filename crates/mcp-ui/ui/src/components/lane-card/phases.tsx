import type { Phase } from '../../utils/lanes';
import { Glyph } from '../glyph';
import { Pane } from '../window';

/** The start phases in a notched pane. */
export function Phases({ phases }: { phases: Phase[] }) {
  return (
    <Pane title="start" cls="phases">
      <ul>
        {phases.map((phase) => (
          <li key={phase.name}>
            <Glyph state={phase.state} />
            <span class="ph-name">{phase.name}</span>
            <span class="ph-detail">{phase.detail}</span>
          </li>
        ))}
      </ul>
    </Pane>
  );
}
