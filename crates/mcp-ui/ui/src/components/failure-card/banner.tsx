import { readOnly } from '../../store/card';
import { actorLabel, reasonText, since } from '../../utils/format';
import { unitState } from '../../utils/state';
import { Glyph } from '../glyph';
import { liveUnit, type Subject } from './subjects';

/**
 * What failed and why: state, exit, when, who started it, the reasons and
 * where to look next. The snapshot's summary wins over the tool result's.
 */
export function Banner({ subject, unitRef }: { subject: Subject; unitRef: string }) {
  const unit = liveUnit(subject);
  const state = unit
    ? unitState(unit)
    : subject.state || (subject.exit != null || subject.tail ? 'failed' : '');
  const exit = unit ? unit.exit_code : subject.exit;
  const now = Date.now();
  const facts: string[] = [];
  if (exit != null && state !== 'ready') facts.push(`exit ${exit}`);
  if (unit?.finished_at_ms) facts.push(`${since(now - unit.finished_at_ms)} ago`);
  if (unit?.finished_at_ms && unit.started_at_ms) {
    facts.push(`after ${since(unit.finished_at_ms - unit.started_at_ms)}`);
  }
  if (unit?.actor) facts.push(`started by ${actorLabel(unit.actor)}`);
  if (subject.crashes)
    facts.push(`${subject.crashes} ${subject.crashes === 1 ? 'crash' : 'crashes'}`);
  const given = subject.reasons || [];
  const reasons = [...given, ...(unit?.reason && !given.length ? [unit.reason] : [])];
  const failed = state === 'failed';
  if (!state) return null;
  return (
    <div class={`banner${failed ? ' failure' : ''}`}>
      <p>
        <Glyph state={state} /> <b class={failed ? 'tone-failure' : ''}>{state}</b>
        {facts.length > 0 && (
          <>
            {' '}
            <span class="dim">· {facts.join(' · ')}</span>
          </>
        )}
      </p>
      {reasons.length > 0 && (
        <ul>
          {reasons.map((reason) => (
            <li key={reason.code}>
              <span class="dim">reason</span> {reasonText(reason)}
            </li>
          ))}
        </ul>
      )}
      {(subject.conflicts || []).map((c) => (
        <p key={c.port}>
          <span class="dim">port</span> {c.port} is held by{' '}
          {c.pid ? `pid ${c.pid}` : 'another process'}
        </p>
      ))}
      {failed && (
        <p>
          <span class="dim">next</span> {!readOnly() && 'restart it, or '}
          <code>devlanes logs {unitRef} --tail 200</code>
        </p>
      )}
    </div>
  );
}
