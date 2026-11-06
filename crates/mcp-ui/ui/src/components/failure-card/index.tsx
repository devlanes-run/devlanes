// The failure card: why a unit failed, the last lines that explain it and two buttons.
// Shown for failed starts, debug why, debug errors and a plain `logs UNIT`. Fullscreen
// swaps the excerpt for the whole log with a filter, earlier pages and line picking.

import { useEffect, useState } from 'preact/hooks';
import { act, ask, busyLabel, display } from '../../store/actions';
import {
  bridge,
  canFullscreen,
  canHandOff,
  card,
  cwd,
  isFullscreen,
  readOnly,
  structured
} from '../../store/card';
import type { LogLine } from '../../types/proto';
import type { LogsResult } from '../../types/results';
import { actorLabel, reasonText, since, unitRef } from '../../utils/format';
import { capital, toneOf, unitState } from '../../utils/state';
import { Button, LinkButton } from '../button';
import { Actions, Card, Head, Line, Pill, Shape } from '../card';
import { LogViewer } from '../log-viewer';
import { TextCard } from '../text-card';
import { Excerpt, excerptOf, lastError } from './excerpt';
import { liveUnit, type Subject, subjectsOf } from './subjects';

/** Lines the card reads when the result carries none with cursors. */
const TAIL = 200;

/** The failure card. */
export function FailureCard() {
  const subjects = subjectsOf(structured());
  const subject = subjects[0];
  if (!subject) {
    const sc = structured() || {};
    // A command that failed before it had a unit to show, such as an unknown lane.
    if (sc.exit_code || sc.data?.error) return <TextCard />;
    return (
      <Card label="failures">
        <Head
          title={`devlanes ${sc.command || ''}`}
          pill={<Pill tone="success">No failures</Pill>}
        />
      </Card>
    );
  }
  return (
    <Failure
      key={unitRef(subject.lane || '', subject.unit)}
      subject={subject}
      others={subjects.slice(1)}
    />
  );
}

/** The lines a subject brings, as log lines: a logs page's own, or a start's plain tail. */
function linesOf(subject: Subject): LogLine[] {
  return (
    subject.lines ||
    (subject.tail || []).map((text) => ({ unit: subject.unit, seq: null, text, level: null }))
  );
}

/** One failed unit, and the other failed units of the result as rows under it. */
function Failure({ subject, others }: { subject: Subject; others: Subject[] }) {
  const ref = unitRef(subject.lane || '', subject.unit);
  const [lines, setLines] = useState<LogLine[]>(() => linesOf(subject));
  const [earlier, setEarlier] = useState<string | null>(subject.next || null);

  // Lines without cursors (a start's tail, or none for `debug why`) are replaced by the
  // unit's own log once, so the excerpt and the viewer agree.
  useEffect(() => {
    if (lines.some((line) => line.seq != null)) return undefined;
    let gone = false;
    bridge
      .callTool<LogsResult>('app_logs', { cwd: cwd(), units: [ref], tail: TAIL })
      .then((result) => {
        const page = result.structuredContent?.logs;
        if (gone || !page?.items?.length) return;
        setLines(page.items);
        setEarlier(page.next_cursor || null);
      })
      .catch(() => {});
    return () => {
      gone = true;
    };
  }, [ref]);

  const unit = liveUnit(subject);
  const state = unit
    ? unitState(unit)
    : subject.state || (subject.exit != null || subject.tail ? 'failed' : 'idle');
  const failed = state === 'failed';
  const exit = unit ? unit.exit_code : subject.exit;
  const now = Date.now();
  const meta = [
    exit != null && failed && `Exit ${exit}`,
    unit?.finished_at_ms && `${since(now - unit.finished_at_ms)} ago`,
    unit?.finished_at_ms &&
      unit.started_at_ms &&
      `after ${since(unit.finished_at_ms - unit.started_at_ms)}`,
    unit?.actor && `started by ${actorLabel(unit.actor)}`,
    subject.crashes && `${subject.crashes} ${subject.crashes === 1 ? 'crash' : 'crashes'}`
  ];
  const given = subject.reasons || [];
  const reasons = given.length ? given : unit?.reason ? [unit.reason] : [];
  const reason = reasons.map(reasonText).filter(Boolean).join('; ');
  const tone = toneOf(state);
  const pill = (
    <Pill tone={tone === 'neutral' && state === 'ready' ? 'success' : tone}>
      {capital(state.replace(/_/g, ' '))}
    </Pill>
  );

  const restart = !readOnly() && (
    <Button
      key="restart"
      label={busyLabel(ref, `Restart ${subject.unit}`, subject.unit)}
      disabled={!!card.busy[ref]}
      onClick={() => act('restart', ref, `restarted ${ref}`)}
    />
  );

  if (isFullscreen()) {
    return (
      <Card label={`logs of ${ref}`}>
        <Head title={ref} pill={pill} meta={meta} />
        <LogViewer
          unitRef={ref}
          lines={lines}
          setLines={setLines}
          earlier={earlier}
          setEarlier={setEarlier}
          restart={restart}
        />
      </Card>
    );
  }

  const shown = excerptOf(lines);
  const error = lastError(lines);
  const evidence = error?.text || reason;
  const total = Math.max(lines.length, ...lines.map((line) => line.seq ?? 0));
  const summary = shown.length > 0 && (
    <>
      {total > shown.length ? `Last ${shown.length} of ${total} lines` : `${shown.length} lines`}
      {canFullscreen() && (
        <>
          {' · '}
          <LinkButton onClick={() => display('fullscreen')}>All lines</LinkButton>
        </>
      )}
    </>
  );
  return (
    <Card label={`failure of ${ref}`}>
      <Head title={ref} pill={pill} meta={meta} />
      {reason && <p class="reason">{capital(reason)}.</p>}
      {(subject.conflicts || []).map((c) => (
        <Line key={c.port} tone="warn">
          Port {c.port} is held by {c.pid ? `pid ${c.pid}` : 'another process'}.
        </Line>
      ))}
      {shown.length > 0 && <Excerpt lines={shown} label={`last lines of ${ref}`} />}
      {others.length > 0 && (
        <ul class="rows">
          {others.map((other) => {
            const why =
              (other.reasons || []).map(reasonText).join('; ') ||
              (other.exit != null ? `exit ${other.exit}` : other.tail?.[other.tail.length - 1]);
            return (
              <li class="row" key={other.unit}>
                <Shape state={other.state || 'failed'} />
                <span class="name">{unitRef(other.lane || '', other.unit)}</span>
                <span class="grow danger">{why}</span>
              </li>
            );
          })}
        </ul>
      )}
      <Actions summary={summary}>
        {restart}
        {failed && canHandOff() && (
          <Button
            key="ask"
            label="Ask agent why"
            kind="primary"
            onClick={() =>
              ask(
                `${subject.unit} in ${subject.lane || 'this lane'} failed with: ${evidence || 'an error'}. Why?`
              )
            }
          />
        )}
      </Actions>
    </Card>
  );
}
