import { act, openUrl } from '../../store/actions';
import { card, readOnly, update } from '../../store/card';
import type { LaneSummary, UnitSummary } from '../../types/proto';
import { shortUrl, unitRef } from '../../utils/format';
import { cpuHistory, portOf, servicesOf } from '../../utils/lanes';
import { isBusy, isQuiet, unitState } from '../../utils/state';
import { Button } from '../button';
import { Glyph } from '../glyph';
import { Spark } from '../spark';

/**
 * One row per service: glyph, name (opens its logs), state when it needs a
 * word, port, URL, CPU and buttons.
 */
export function Units({ lane }: { lane: LaneSummary }) {
  const units = servicesOf(lane);
  if (!units.length) {
    return (
      <p class="dim">
        no services; {lane.state === 'idle' ? 'start the lane to run them' : 'this lane runs none'}
      </p>
    );
  }
  return (
    <table class="units" aria-label={`services of ${lane.reference}`}>
      <tbody>
        {units.map((unit) => (
          <UnitRow key={unit.name} lane={lane} unit={unit} />
        ))}
      </tbody>
    </table>
  );
}

/** One service of {@link Units}. */
function UnitRow({ lane, unit }: { lane: LaneSummary; unit: UnitSummary }) {
  const ref = unitRef(lane.reference, unit.name);
  const busy = card.busy[ref];
  const state = busy ? 'starting' : unitState(unit);
  const port = portOf(unit);
  const live = unit.state === 'ready' || isBusy(unit.state);
  const url = unit.public_url;
  // A failed unit restarts; a stopped one starts.
  const revive = state === 'failed' ? 'restart' : 'start';
  return (
    <tr>
      <td class="g">
        <Glyph state={state} />
      </td>
      <th scope="row">
        <button
          type="button"
          class="link name"
          title={`logs of ${ref}`}
          onClick={() => update({ drill: { ...card.drill, unit: ref } })}
        >
          {unit.display_name || unit.name}
        </button>
        {!isQuiet(state) && (
          <>
            {' '}
            <span class={`word tone-${state === 'failed' ? 'failure' : 'plain'}`}>
              {busy ? `${busy}…` : state}
              {unit.exit_code != null && state !== 'done' && (
                <span class="exit"> · exit {unit.exit_code}</span>
              )}
            </span>
          </>
        )}
      </th>
      <td class="port dim">{port && `:${port}`}</td>
      <td class="url">
        {url && unit.state === 'ready' && (
          <button type="button" class="link url" onClick={() => openUrl(url)}>
            {shortUrl(url)}
          </button>
        )}
      </td>
      <td class="cpu">
        <Spark
          values={cpuHistory(card.snapshot?.top, lane.reference, unit.name)}
          label={`${unit.name} cpu`}
        />
      </td>
      {!readOnly() && (
        <td class="act">
          {live ? (
            <>
              <Button
                icon="restart"
                label={`restart ${unit.name}`}
                iconOnly
                disabled={!!busy}
                onClick={() => act('restart', ref, `restarted ${ref}`)}
              />
              <Button
                icon="stop"
                label={`stop ${unit.name}`}
                iconOnly
                disabled={!!busy}
                onClick={() => act('stop', ref, `stopped ${ref}`)}
              />
            </>
          ) : (
            <Button
              icon={revive}
              label={`${revive} ${unit.name}`}
              iconOnly
              disabled={!!busy}
              onClick={() => act(revive, ref, `${revive}ed ${ref}`)}
            />
          )}
        </td>
      )}
    </tr>
  );
}
