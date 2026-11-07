import type { CleanPlan } from '../../types/clean';
import type { LaneSummary } from '../../types/proto';
import { actorLabel, atIndex, size } from '../../utils/format';
import { Pill, Shape } from '../card';

/** A lane's plan, or why the card could not read it. */
export type PlanOrError = CleanPlan | { error: string };

/** Why a clean would skip a lane's tasks and teardown, as `lane clean --dry-run` says it. */
const SKIPPED: Record<string, string> = {
  worktree_missing: "The worktree is gone, so teardown won't run",
  needs_env: "Needs a start from a shell first, so teardown won't run",
  unassigned: "The worktree has no lane, so teardown won't run"
};

/** The bytes a plan frees, or null when the daemon knows none of its sizes. */
export function planBytes(plan: CleanPlan): number | null {
  const sizes = [
    ...(plan.volumes || []).map((v) => v.bytes),
    plan.data_dir?.bytes,
    plan.logs_bytes
  ];
  const known = sizes.filter((n): n is number => typeof n === 'number');
  return known.length ? known.reduce((a, b) => a + b, 0) : null;
}

/** A volume's name without the compose project or lane prefix the whole row already names. */
function volumeName(plan: CleanPlan, name: string): string {
  const prefix = plan.compose_project ? `${plan.compose_project}_` : '';
  return prefix && name.startsWith(prefix) ? name.slice(prefix.length) : name;
}

/** The one thing to know before cleaning a lane, in the warning tone, or null. */
function catchOf(plan: CleanPlan): string | null {
  const running = plan.running || [];
  if (running.length) {
    const units = running.map((r) => r.unit.split(':').pop()).join(', ');
    const by = [...new Set(running.map((r) => actorLabel(r.actor)))].join(', ');
    return `Stops ${units} started by ${by}`;
  }
  if (plan.hooks_skipped)
    return SKIPPED[plan.hooks_skipped] || plan.hooks_skipped.replace(/_/g, ' ');
  if (plan.infra_error) return `Containers unknown: ${plan.infra_error}`;
  return null;
}

/** The props of {@link PlanRow}. */
interface PlanRowProps {
  lane: Pick<LaneSummary, 'reference'> & Partial<LaneSummary>;
  plan: PlanOrError | undefined;
  /** `cleaning`, `cleaned` or `failed: <why>` once the person cleaned it. */
  result: string | undefined;
  checked: boolean;
  ro: boolean;
  onToggle: () => void;
}

/**
 * One lane: checkbox, `@4`, branch and size, then one line of what goes or the catch,
 * and from 640 px the teardown and tasks a clean runs. A cleaned lane keeps only its pill.
 */
export function PlanRow({ lane, plan, result, checked, ro, onToggle }: PlanRowProps) {
  const id = `clean-${lane.reference.replace(/[^\w-]/g, '-')}`;
  const refused = plan && 'error' in plan ? plan.error : null;
  const good = plan && !('error' in plan) ? plan : null;
  const bytes = good ? planBytes(good) : null;
  const branch = lane.name || lane.branch || good?.branch || '';
  if (result === 'cleaned' || result === 'cleaning') {
    return (
      <li class="row">
        {result === 'cleaning' && <Shape state="cleaning" />}
        <span class="name num">{atIndex(lane.reference)}</span>
        <span class="grow sec">{branch}</span>
        <span class="end">
          {result === 'cleaned' ? (
            <Pill tone="success">Cleaned</Pill>
          ) : (
            <span class="sec">cleaning…</span>
          )}
        </span>
      </li>
    );
  }
  const goes: string[] = [];
  if (good) {
    for (const v of good.volumes || [])
      goes.push(`${volumeName(good, v.name)} ${size(v.bytes)}`.trim());
    if (good.data_dir) goes.push(`data ${size(good.data_dir.bytes)}`.trim());
    if (good.logs_bytes) goes.push(`logs ${size(good.logs_bytes)}`);
    const containers = good.containers?.length ?? 0;
    if (containers) goes.push(`${containers} ${containers === 1 ? 'container' : 'containers'}`);
  }
  const steps = good
    ? [
        ...(good.teardown || []).map((t) => `teardown ${t.name}`),
        ...(good.tasks || []).map((t) => `task ${t.name}`)
      ]
    : [];
  const caught = good ? catchOf(good) : null;
  const failed = result?.startsWith('failed') ? result.slice('failed: '.length) : null;
  const line = failed || refused?.split('\n')[0]?.replace(/^error: /, '') || caught;
  return (
    <li class={`row${refused ? ' dim' : ''}${ro ? ' plain' : ' check'}`}>
      {!ro && (
        <input
          type="checkbox"
          id={id}
          checked={checked}
          disabled={!plan || !!refused}
          onChange={onToggle}
          aria-label={`Clean ${lane.reference}`}
        />
      )}
      <label class="name num" for={id}>
        {atIndex(lane.reference)}
      </label>
      <span class="grow sec">{branch}</span>
      <span class="end num">{size(bytes)}</span>
      <span class="line2 meta">
        {!plan ? (
          'Reading what a clean deletes…'
        ) : line ? (
          <span class={failed ? 'danger' : refused ? '' : 'warn'}>{line}</span>
        ) : (
          goes.join(' · ')
        )}
        {steps.length > 0 && !line && <span class="w640"> · runs {steps.join(', ')}</span>}
      </span>
    </li>
  );
}
