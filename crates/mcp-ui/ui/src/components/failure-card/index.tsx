// The failure card: why a unit failed, who started it, and its logs. Shown for
// failed starts, debug why, debug errors and logs, and for a unit opened from a
// lane card.

import { useState } from 'preact/hooks';
import { card, structured, update } from '../../store/card';
import { unitRef } from '../../utils/format';
import { Button } from '../button';
import { FullscreenButton } from '../fullscreen-button';
import { Glyph } from '../glyph';
import { LogViewer } from '../log-viewer';
import { PollStatus } from '../poll-status';
import { TextCard } from '../text-card';
import { Window } from '../window';
import { Banner } from './banner';
import { subjectsOf } from './subjects';

/** The failure card. */
export function FailureCard() {
  const subjects = subjectsOf(structured());
  const [index, setIndex] = useState(0);
  const back = card.drill?.unit ? (
    <Button
      icon="back"
      label="back"
      iconOnly
      onClick={() => update({ drill: card.drill?.lane ? { lane: card.drill.lane } : null })}
    />
  ) : null;
  const tools = (
    <>
      {back}
      <FullscreenButton />
    </>
  );
  const subject = subjects[Math.min(index, subjects.length - 1)];
  if (!subject) {
    const sc = structured() || {};
    // A command that failed before it had a unit to show, such as an unknown lane.
    if (sc.exit_code || sc.data?.error) return <TextCard />;
    const command = sc.command || '';
    return (
      <Window title={<b>devlanes {command}</b>} label="failures" tools={tools}>
        <p class="status-line">
          <Glyph state="ready" /> {command === 'logs' ? 'no lines' : 'nothing failed'}
        </p>
      </Window>
    );
  }
  const ref = unitRef(subject.lane || '', subject.unit);
  return (
    <Window title={<b>{ref}</b>} label={`failure of ${ref}`} tools={tools} foot={<PollStatus />}>
      {subjects.length > 1 && (
        <div class="tabs" role="tablist" aria-label="units">
          {subjects.map((s, i) => {
            const tab = unitRef(s.lane || '', s.unit);
            return (
              <button
                key={tab}
                type="button"
                role="tab"
                class="tab"
                aria-selected={i === index}
                onClick={() => setIndex(i)}
              >
                {tab}
              </button>
            );
          })}
        </div>
      )}
      <Banner subject={subject} unitRef={ref} />
      <LogViewer key={ref} subject={subject} unitRef={ref} />
    </Window>
  );
}
