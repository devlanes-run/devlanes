// The lane card: the lane's state as a pill, its start progress while it comes up, one
// row per service with its URL, and at most two buttons. Shown for start, restart,
// reload and the status of one lane.

import { useEffect, useState } from 'preact/hooks';
import { act, ask, busyLabel, openUrl } from '../../store/actions';
import {
  bridge,
  callRunning,
  canHandOff,
  card,
  cwd,
  findLane,
  readOnly,
  resultLane,
  structured
} from '../../store/card';
import type { LaneSummary, LogLine } from '../../types/proto';
import type { LogsResult } from '../../types/results';
import { reasonText, unitRef } from '../../utils/format';
import {
  actorsOf,
  domainService,
  laneState,
  laneWords,
  progressOf,
  servicesOf
} from '../../utils/lanes';
import { isBusy } from '../../utils/state';
import { Button } from '../button';
import { Actions, Card, Head, Line, Pill, Skeleton } from '../card';
import { lastError } from '../failure-card/excerpt';
import { TextCard } from '../text-card';
import { Units } from './units';

/** The pill of a lane: its state in words and tone. */
export function lanePill(lane: LaneSummary, now: number) {
  const state = laneState(lane);
  if (state === 'failed') return <Pill tone="danger">Failed</Pill>;
  if (state === 'running') return <Pill tone="success">Ready</Pill>;
  if (state === 'frozen' || state === 'throttled') {
    const word = laneWords(lane, now);
    return <Pill tone="info">{word.charAt(0).toUpperCase() + word.slice(1)}</Pill>;
  }
  const word = state.replace(/_/g, ' ');
  return <Pill>{word.charAt(0).toUpperCase() + word.slice(1)}</Pill>;
}

/**
 * The last error line of each failed unit, read once, so "Ask agent why" hands the
 * agent the evidence along with the reason.
 */
function useErrorLines(lane: LaneSummary | null, failed: string[]): Record<string, string> {
  const [lines, setLines] = useState<Record<string, string>>({});
  const key = failed.join(',');
  useEffect(() => {
    if (!lane || !failed.length || readOnly()) return undefined;
    let gone = false;
    for (const name of failed) {
      if (lines[name] !== undefined) continue;
      bridge
        .callTool<LogsResult>('app_logs', {
          cwd: cwd(),
          units: [unitRef(lane.reference, name)],
          tail: 50
        })
        .then((result) => {
          const items: LogLine[] = result.structuredContent?.logs?.items || [];
          if (!gone) setLines((old) => ({ ...old, [name]: lastError(items)?.text || '' }));
        })
        .catch(() => {});
    }
    return () => {
      gone = true;
    };
  }, [lane?.reference, key]);
  return lines;
}

/** The lane card. */
export function LaneCard() {
  const lane = findLane(resultLane(structured()));
  const services = lane ? servicesOf(lane) : [];
  const failed = services.filter((unit) => unit.state === 'failed');
  const errors = useErrorLines(
    lane,
    failed.map((unit) => unit.name)
  );
  if (!lane) return card.result ? <TextCard /> : <Skeleton label="lane" />;

  const now = Date.now();
  const ref = lane.reference;
  const state = laneState(lane);
  const progress = isBusy(lane.state) ? progressOf(lane) : null;
  const actors = actorsOf(services);
  const ro = readOnly();
  const running = callRunning();

  const notes: string[] = [];
  if (lane.needs_env)
    notes.push('The lane lost its environment; a start from a shell brings it back.');
  if (lane.config_changed) notes.push('devlanes.yaml changed since the lane started.');
  for (const warning of lane.warnings || []) notes.push(warning.data?.message || warning.code);
  if (card.cancelled) notes.push('The call was cancelled; services keep starting.');

  const buttons = [];
  if (!running) {
    if (failed.length) {
      const [first] = failed;
      // One failed service restarts alone; several restart with their lane.
      const one = failed.length === 1 ? first : undefined;
      const target = one ? unitRef(ref, one.name) : ref;
      if (!ro) {
        buttons.push(
          <Button
            key="restart"
            label={busyLabel(target, one ? `Restart ${one.name}` : 'Restart lane', one?.name)}
            disabled={!!card.busy[target]}
            onClick={() => act('restart', target, `restarted ${target}`)}
          />
        );
      }
      if (canHandOff() && first) {
        const reason = reasonText(first.reason);
        const error = errors[first.name];
        const evidence = error ? `${error}${reason ? ` (${reason})` : ''}` : reason || 'an error';
        buttons.push(
          <Button
            key="ask"
            label="Ask agent why"
            kind="primary"
            onClick={() => ask(`${first.name} in ${ref} failed with: ${evidence}. Why?`)}
          />
        );
      }
    } else if (['idle', 'stopped'].includes(state)) {
      if (!ro) {
        buttons.push(
          <Button
            key="start"
            label={busyLabel(ref, 'Start lane', ref)}
            kind="primary"
            disabled={!!card.busy[ref]}
            onClick={() => act('start', ref, `started ${ref}`)}
          />
        );
      }
    } else {
      if (!ro) {
        const reload = lane.config_changed;
        buttons.push(
          <Button
            key="stop"
            label={busyLabel(ref, reload ? 'Reload' : 'Stop lane', ref)}
            disabled={!!card.busy[ref]}
            onClick={() =>
              reload ? act('reload', ref, `reloaded ${ref}`) : act('stop', ref, `stopped ${ref}`)
            }
          />
        );
      }
      const web = domainService(lane);
      if (web?.public_url) {
        const url = web.public_url;
        const ready = web.state === 'ready';
        buttons.push(
          <Button
            key="open"
            label={ready ? 'Open' : `Open when ${web.name} is ready`}
            kind="primary"
            disabled={!ready}
            title={url}
            onClick={() => openUrl(url)}
          />
        );
      }
    }
  }

  return (
    <Card label={`lane ${ref}`}>
      <Head
        title={ref}
        sub={lane.name || lane.branch}
        pill={lanePill(lane, now)}
        meta={[actors.length > 0 && `started by ${actors.join(', ')}`]}
      />
      {progress && (
        <>
          <p class="phase">
            <span>{progress.label}</span>
            <span class="num">
              {progress.step} of {progress.steps}
            </span>
          </p>
          <div
            class="bar"
            role="progressbar"
            aria-label="start progress"
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={Math.round(progress.share * 100)}
          >
            <i style={{ width: `${Math.max(4, progress.share * 100)}%` }} />
          </div>
        </>
      )}
      {notes.length > 0 && <Line tone="warn">{notes.join(' ')}</Line>}
      <Units lane={lane} waiting={isBusy(lane.state)} />
      <Actions>{buttons}</Actions>
    </Card>
  );
}
