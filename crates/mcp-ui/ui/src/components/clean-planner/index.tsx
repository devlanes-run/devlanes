// The clean planner: the project's lanes, what cleaning each one deletes (from
// `lane clean --dry-run`), checkboxes and a confirm inside the card. The click
// on the confirm is the consent, so `clean` runs with --yes on the server.

import { useEffect, useState } from 'preact/hooks';
import { act, ask, tellModel } from '../../store/actions';
import { bridge, card, cwd, readOnly, structured } from '../../store/card';
import type { CleanPlan } from '../../types/clean';
import type { LaneSummary } from '../../types/proto';
import type { ActionResult, Snapshot } from '../../types/results';
import { size } from '../../utils/format';
import { Button } from '../button';
import { FullscreenButton } from '../fullscreen-button';
import { Note } from '../note';
import { Window } from '../window';
import { type PlanOrError, PlanRow, planBytes } from './plan-row';

/** A lane row of the planner: a snapshot lane, or the target alone until lanes arrive. */
type PlanLane = Pick<LaneSummary, 'reference'> & Partial<LaneSummary>;

/** The clean planner. */
export function CleanPlanner() {
  const sc = structured() || {};
  const args = sc.args || [];
  const dryRun = args.includes('--dry-run');
  const target: string | undefined =
    sc.data?.lane || args.filter((a) => !a.startsWith('-') && a !== 'lane' && a !== 'clean')[0];
  const [lanes, setLanes] = useState<LaneSummary[] | null>(null);
  const [plans, setPlans] = useState<Record<string, PlanOrError>>(
    dryRun && sc.data ? { [sc.data.lane]: sc.data as CleanPlan } : {}
  );
  const [checked, setChecked] = useState<string[]>(target && dryRun ? [target] : []);
  const [results, setResults] = useState<Record<string, string>>(
    !dryRun && target && !sc.exit_code ? { [target]: 'cleaned' } : {}
  );
  const [confirm, setConfirm] = useState(false);
  const [error, setError] = useState('');
  // Read-only servers offer no app_action, so the card shows only the plan the result holds.
  const ro = readOnly();

  // Once, when the card opens: later plans would size every volume again.
  useEffect(() => {
    if (ro) return undefined;
    let gone = false;
    (async () => {
      try {
        const snap: Partial<Snapshot> =
          (await bridge.callTool<Snapshot>('app_snapshot', { cwd: cwd() })).structuredContent || {};
        if (snap.daemon === 'stopped') throw new Error('the daemon is not running');
        const items = (snap.status?.items || [])
          .slice()
          .sort((a, b) => (a.index ?? 0) - (b.index ?? 0));
        if (gone) return;
        setLanes(items);
        // One plan at a time: each one sizes volumes and folders on disk.
        for (const lane of items) {
          if (gone || plans[lane.reference] || results[lane.reference]) continue;
          const out: Partial<ActionResult> = await bridge
            .callTool<ActionResult>('app_action', {
              cwd: cwd(),
              action: 'clean_plan',
              target: lane.reference
            })
            .then(
              (result) => result.structuredContent || {},
              (e: Error) => ({ exit_code: 1, text: e.message })
            );
          if (gone) return;
          setPlans((old) => ({
            ...old,
            [lane.reference]:
              !out.exit_code && out.data ? out.data : { error: out.text || 'no plan' }
          }));
        }
      } catch (e) {
        if (!gone) setError((e as Error).message);
      }
    })();
    return () => {
      gone = true;
    };
  }, []);

  const targetPlan = target ? plans[target] : undefined;
  const planned = targetPlan && !('error' in targetPlan) ? targetPlan : undefined;
  const rows: PlanLane[] =
    lanes ||
    (target
      ? [{ reference: target, index: planned?.index ?? null, branch: planned?.branch ?? null }]
      : []);
  const chosen = checked.filter((ref) => !results[ref]);
  const total = chosen.reduce((sum, ref) => {
    const plan = plans[ref];
    return sum + ((plan && !('error' in plan) && planBytes(plan)) || 0);
  }, 0);
  const toggle = (ref: string) =>
    setChecked((old) => (old.includes(ref) ? old.filter((r) => r !== ref) : [...old, ref]));

  const clean = async () => {
    setConfirm(false);
    const done: string[] = [];
    for (const ref of chosen) {
      setResults((old) => ({ ...old, [ref]: 'cleaning' }));
      const out = await act('clean', ref);
      const ok = out && !out.exit_code;
      const first = String(out?.text || '').split('\n')[0];
      setResults((old) => ({ ...old, [ref]: ok ? 'cleaned' : `failed: ${first}` }));
      if (ok) done.push(ref);
    }
    if (done.length) tellModel(`user cleaned ${done.join(', ')} from the card`);
  };

  const question = chosen.length
    ? `Before I clean ${chosen.join(', ')}: is anything there not pushed or not merged?`
    : 'Which of my lanes are safe to clean?';
  const foot = (
    <>
      <span class="poll">
        {chosen.length ? `frees about ${size(total) || '0 B'}` : 'pick the lanes to clean'}
      </span>
      <span class="actions">
        <Button icon="ask" label="ask agent first" onClick={() => ask(question)} />
        {!ro && (
          <Button
            label={`clean ${chosen.length || ''} ${chosen.length === 1 ? 'lane' : 'lanes'}`.replace(
              '  ',
              ' '
            )}
            primary
            disabled={!chosen.length || confirm}
            onClick={() => setConfirm(true)}
          />
        )}
      </span>
    </>
  );

  const project = rows[0]?.project;
  const title = (
    <>
      <b>clean lanes</b>
      {project && (
        <>
          {' '}
          <span class="dim">· {project}</span>
        </>
      )}
    </>
  );
  return (
    <Window title={title} label="clean planner" tools={<FullscreenButton />} foot={foot}>
      {error && (
        <p class="note tone-failure" role="status">
          {error}
        </p>
      )}
      {rows.length === 0 && !error && <p class="dim">loading lanes…</p>}
      <ul class="plans">
        {rows.map((lane) => (
          <PlanRow
            key={lane.reference}
            lane={lane}
            plan={plans[lane.reference]}
            result={results[lane.reference]}
            checked={checked.includes(lane.reference)}
            ro={ro}
            onToggle={() => toggle(lane.reference)}
          />
        ))}
      </ul>
      {confirm && (
        <div class="banner confirm" role="alertdialog" aria-label="confirm clean">
          <p>
            <b>clean {chosen.join(', ')}?</b>
          </p>
          <p>
            Runs their clean tasks and teardown, deletes the volumes and data folders listed above
            and frees their ports. This can't be undone.
          </p>
          <p class="actions">
            <Button label="cancel" onClick={() => setConfirm(false)} />
            <Button label={`clean ${chosen.length}`} primary onClick={clean} />
          </p>
        </div>
      )}
      <Note note={card.note} />
    </Window>
  );
}
