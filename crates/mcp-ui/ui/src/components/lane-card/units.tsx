import { useState } from 'preact/hooks';
import { openUrl } from '../../store/actions';
import { daemonStopped, isFullscreen } from '../../store/card';
import type { LaneSummary, UnitSummary } from '../../types/proto';
import { reasonText, shortUrl } from '../../utils/format';
import { byAttention, portOf, servicesOf } from '../../utils/lanes';
import { capital, unitState, wordClass, words } from '../../utils/state';
import { LinkButton } from '../button';
import { Line, Shape } from '../card';

/** Rows an inline card shows before the rest fold into one line. */
const SHOWN = 15;

/**
 * One row per service: shape, name, URL link, port from 520 px, state word. A failed
 * service says why on a second line. Failed and starting services come first; past
 * {@link SHOWN} rows the rest fold into one line with "Show all", which grows the
 * card instead of scrolling it, since hosts discourage scrolling inside a card.
 * Fullscreen shows every row.
 */
export function Units({ lane }: { lane: LaneSummary }) {
  const [all, setAll] = useState(false);
  const units = byAttention(servicesOf(lane));
  if (!units.length) return <Line>No services.</Line>;
  const folds = units.length > SHOWN && !isFullscreen();
  const shown = folds && !all ? units.slice(0, SHOWN) : units;
  return (
    <ul class="rows" aria-label={`services of ${lane.reference}`}>
      {shown.map((unit) => (
        <UnitRow key={unit.name} unit={unit} />
      ))}
      {folds && (
        <li class="row">
          <span class="grow sec">
            {!all && `${moreText(units.slice(SHOWN))} · `}
            <LinkButton expanded={all} onClick={() => setAll(!all)}>
              {all ? 'Show fewer' : 'Show all'}
            </LinkButton>
          </span>
        </li>
      )}
    </ul>
  );
}

/** The folded rows in words: `28 more services, all idle` or `28 more: 3 ready, 25 idle`. */
function moreText(rest: UnitSummary[]): string {
  const counts = new Map<string, number>();
  for (const unit of rest) {
    const word = words(unitState(unit));
    counts.set(word, (counts.get(word) || 0) + 1);
  }
  const parts = [...counts].map(([word, count]) => `${count} ${word}`);
  const [only] = counts.keys();
  if (counts.size === 1 && only !== undefined) {
    return rest.length === 1
      ? `1 more service, ${only}`
      : `${rest.length} more services, all ${only}`;
  }
  return `${rest.length} more: ${parts.join(', ')}`;
}

/** One service of {@link Units}. */
function UnitRow({ unit }: { unit: UnitSummary }) {
  const state = unitState(unit);
  const port = portOf(unit);
  const url = unit.public_url;
  const failed = state === 'failed';
  const why = failed
    ? [capital(reasonText(unit.reason)), unit.exit_code != null && `Exit ${unit.exit_code}`]
        .filter(Boolean)
        .join(' · ')
    : '';
  return (
    <li class="row">
      <Shape state={state} />
      <span class="name">{unit.display_name || unit.name}</span>
      <span class="grow">
        {url && unit.state === 'ready' && !daemonStopped() ? (
          <LinkButton title={url} onClick={() => openUrl(url)}>
            {shortUrl(url)}
          </LinkButton>
        ) : (
          url && <span class="ter">{shortUrl(url)}</span>
        )}
      </span>
      {port && <span class="col num w520">:{port}</span>}
      <span class={`end ${wordClass(state)}`}>{words(state)}</span>
      {why && <span class="line2 meta danger">{why}</span>}
    </li>
  );
}
