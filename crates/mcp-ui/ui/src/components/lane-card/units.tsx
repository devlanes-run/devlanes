import { openUrl } from '../../store/actions';
import { daemonStopped } from '../../store/card';
import type { LaneSummary, UnitSummary } from '../../types/proto';
import { reasonText, shortUrl } from '../../utils/format';
import { portOf, servicesOf } from '../../utils/lanes';
import { capital, unitState, wordClass, words } from '../../utils/state';
import { LinkButton } from '../button';
import { Line, Shape } from '../card';

/**
 * One row per service: shape, name, URL link, port from 520 px, state word. A failed
 * service says why on a second line. `waiting` names the services a start has not
 * reached yet.
 */
export function Units({ lane, waiting }: { lane: LaneSummary; waiting: boolean }) {
  const units = servicesOf(lane);
  if (!units.length) return <Line>No services.</Line>;
  return (
    <ul class="rows" aria-label={`services of ${lane.reference}`}>
      {units.map((unit) => (
        <UnitRow key={unit.name} unit={unit} waiting={waiting} />
      ))}
    </ul>
  );
}

/** One service of {@link Units}. */
function UnitRow({ unit, waiting }: { unit: UnitSummary; waiting: boolean }) {
  const shown = unitState(unit);
  const state = waiting && ['idle', 'stopped'].includes(shown) ? 'waiting' : shown;
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
      <Shape state={state === 'waiting' ? 'idle' : state} />
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
      <span class={`end ${wordClass(state === 'waiting' ? 'idle' : state)}`}>{words(state)}</span>
      {why && <span class="line2 meta danger">{why}</span>}
    </li>
  );
}
