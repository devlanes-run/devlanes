// The clean planner: the project's lanes with what cleaning each one deletes (from
// `lane clean --dry-run`), checkboxes and a confirm in the action bar. The confirm click
// is the consent, so `clean` runs with --yes on the server. The card follows the poll,
// so a lane cleaned through the chat turns into "Cleaned" here too.

import { useEffect, useState } from 'preact/hooks';
import { act, ask, tellModel } from '../../store/actions';
import {
  bridge,
  canHandOff,
  card,
  cwd,
  readOnly,
  snapshotLanes,
  structured
} from '../../store/card';
import type { CleanPlan } from '../../types/clean';
import type { LaneSummary } from '../../types/proto';
import type { ActionResult } from '../../types/results';
import { atIndex, size } from '../../utils/format';
import { Button } from '../button';
import { Actions, Card, Head } from '../card';
import { type PlanOrError, PlanRow, planBytes } from './plan-row';

/** A lane row of the planner: a snapshot lane, or the target alone until lanes arrive. */
type PlanLane = Pick<LaneSummary, 'reference'> & Partial<LaneSummary>;

/** The lanes as one phrase: `@4`, or `@2 and @4`. */
function refs(lanes: string[]): string {
  const short = lanes.map(atIndex);
  return short.length > 1
    ? `${short.slice(0, -1).join(', ')} and ${short[short.length - 1]}`
    : short[0] || '';
}

/** Asks for one lane's dry run. */
async function planOf(ref: string): Promise<PlanOrError> {
  const out: Partial<ActionResult> = await bridge
    .callTool<ActionResult>('app_action', { cwd: cwd(), action: 'clean_plan', target: ref })
    .then(
      (result) => result.structuredContent || {},
      (e: Error) => ({ exit_code: 1, text: e.message })
    );
  return !out.exit_code && out.data ? (out.data as CleanPlan) : { error: out.text || 'No plan' };
}

/** The clean planner. */
export function CleanPlanner() {
  const sc = structured() || {};
  const args = sc.args || [];
  const dryRun = args.includes('--dry-run');
  const target: string | undefined =
    sc.data?.lane || args.filter((a) => !a.startsWith('-') && a !== 'lane' && a !== 'clean')[0];
  const project = sc.data?.project || target?.split('@')[0];
  const [known, setKnown] = useState<PlanLane[]>([]);
  const [plans, setPlans] = useState<Record<string, PlanOrError>>(
    dryRun && sc.data ? { [sc.data.lane]: sc.data as CleanPlan } : {}
  );
  const [checked, setChecked] = useState<string[]>(target && dryRun ? [target] : []);
  const [results, setResults] = useState<Record<string, string>>(
    !dryRun && target && !sc.exit_code ? { [target]: 'cleaned' } : {}
  );
  const [confirm, setConfirm] = useState(false);
  const [freed, setFreed] = useState(0);
  const ro = readOnly();

  // Each snapshot adds the project's new lanes and asks for their plans, all at once; a
  // lane that left the snapshot was cleaned somewhere else.
  useEffect(() => {
    if (ro || !card.snapshot?.status) return;
    const lanes = snapshotLanes()
      .filter((lane) => lane.project === project)
      .sort((a, b) => (a.index ?? 0) - (b.index ?? 0));
    const fresh = lanes.filter((lane) => !known.some((k) => k.reference === lane.reference));
    const gone = known.filter((k) => !lanes.some((lane) => lane.reference === k.reference));
    if (fresh.length)
      setKnown((old) => [...old, ...fresh].sort((a, b) => (a.index ?? 0) - (b.index ?? 0)));
    if (gone.length) {
      setResults((old) => {
        const next = { ...old };
        for (const lane of gone) if (!next[lane.reference]) next[lane.reference] = 'cleaned';
        return next;
      });
    }
    for (const lane of fresh) {
      if (plans[lane.reference] || results[lane.reference]) continue;
      planOf(lane.reference).then((plan) =>
        setPlans((old) => ({ ...old, [lane.reference]: plan }))
      );
    }
  }, [card.snapshotSeq]);

  const rows: PlanLane[] =
    known.length && !ro
      ? known
      : target
        ? [{ reference: target, branch: (plans[target] as CleanPlan | undefined)?.branch ?? null }]
        : [];
  const chosen = checked.filter((ref) => !results[ref]);
  const total = chosen.reduce((sum, ref) => {
    const plan = plans[ref];
    return sum + ((plan && !('error' in plan) && planBytes(plan)) || 0);
  }, 0);
  const cleaning = Object.values(results).includes('cleaning');
  const toggle = (ref: string) => {
    setConfirm(false);
    setChecked((old) => (old.includes(ref) ? old.filter((r) => r !== ref) : [...old, ref]));
  };

  const clean = async () => {
    setConfirm(false);
    const done: string[] = [];
    let bytes = 0;
    for (const ref of chosen) {
      setResults((old) => ({ ...old, [ref]: 'cleaning' }));
      const out = await act('clean', ref);
      const ok = out && !out.exit_code;
      const first = String(out?.text || '')
        .split('\n')[0]
        ?.replace(/^error: /, '');
      setResults((old) => ({ ...old, [ref]: ok ? 'cleaned' : `failed: ${first}` }));
      if (ok) {
        done.push(ref);
        const plan = plans[ref];
        bytes += (plan && !('error' in plan) && planBytes(plan)) || 0;
      }
    }
    setFreed((old) => old + bytes);
    if (done.length) tellModel(`user cleaned ${done.join(', ')} from the card`);
  };

  let footer = null;
  if (!ro && confirm && chosen.length) {
    footer = (
      <Actions
        warn
        summary={`Deletes ${refs(chosen)}'s volumes and data ${chosen.length === 1 ? 'folder' : 'folders'}. Can't be undone.`}
      >
        <Button key="cancel" label="Cancel" onClick={() => setConfirm(false)} />
        <Button
          key="delete"
          label={total ? `Delete ${size(total)}` : `Delete ${refs(chosen)}`}
          kind="destroy"
          onClick={clean}
        />
      </Actions>
    );
  } else if (!ro) {
    const label = cleaning
      ? `Cleaning ${refs(Object.keys(results).filter((r) => results[r] === 'cleaning'))}…`
      : chosen.length > 1
        ? `Clean ${chosen.length} lanes`
        : chosen[0]
          ? `Clean ${atIndex(chosen[0])}`
          : 'Clean';
    footer = (
      <Actions
        summary={
          chosen.length
            ? `Frees ${size(total) || '0 B'}`
            : freed
              ? `Freed ${size(freed)}`
              : undefined
        }
      >
        {canHandOff() && chosen.length > 0 && (
          <Button
            key="ask"
            label="Ask agent first"
            onClick={() =>
              ask(
                `Before I clean ${chosen.join(', ')}: is anything there not pushed or not merged?`
              )
            }
          />
        )}
        <Button
          key="clean"
          label={label}
          kind="danger"
          disabled={!chosen.length || cleaning}
          onClick={() => setConfirm(true)}
        />
      </Actions>
    );
  }

  return (
    <Card label="clean lanes">
      <Head title="Clean lanes" sub={project} />
      <ul class="rows">
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
      {footer}
    </Card>
  );
}
