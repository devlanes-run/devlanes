import type { CleanPlan } from '../../types/clean';
import type { LaneSummary } from '../../types/proto';
import { actorLabel, size } from '../../utils/format';
import { Actors, LaneMark } from '../chips';
import { Glyph } from '../glyph';

/** A lane's plan, or why the card could not read it. */
export type PlanOrError = CleanPlan | { error: string };

/** Why a clean would skip a lane's tasks and teardown, as `lane clean --dry-run` says it. */
const SKIPPED: Record<string, string> = {
  worktree_missing: 'the worktree is gone',
  needs_env: 'the lane needs a start from a shell first',
  unassigned: 'the worktree has no lane'
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

/** One lane with what cleaning it deletes; without a checkbox when `ro`. */
export function PlanRow({ lane, plan, result, checked, ro, onToggle }: PlanRowProps) {
  const id = `clean-${lane.reference.replace(/[^\w-]/g, '-')}`;
  const failedPlan = plan && 'error' in plan ? plan.error : null;
  const good = plan && !('error' in plan) ? plan : null;
  const bytes = good ? planBytes(good) : null;
  const deletes: string[] = [];
  if (good) {
    for (const v of good.volumes || []) {
      deletes.push(`volume ${v.name}${v.bytes != null ? ` ${size(v.bytes)}` : ''}`);
    }
    const containers = good.containers?.length ?? 0;
    if (containers) deletes.push(`${containers} ${containers === 1 ? 'container' : 'containers'}`);
    if (good.data_dir) {
      deletes.push(
        `data folder${good.data_dir.bytes != null ? ` ${size(good.data_dir.bytes)}` : ''}`
      );
    }
    if (good.logs_bytes) deletes.push(`logs ${size(good.logs_bytes)}`);
  }
  const steps = good
    ? [
        ...(good.teardown || []).map((t) => `teardown ${t.name}`),
        ...(good.tasks || []).map((t) => `task ${t.name}`)
      ]
    : [];
  const running = good?.running || [];
  const branch = lane.name || lane.branch || good?.branch || '';
  const done = result === 'cleaned';
  return (
    <li class={`plan${done ? ' done' : ''}${ro ? ' ro' : ''}`}>
      {!ro && (
        <input
          type="checkbox"
          id={id}
          checked={checked && !done}
          disabled={!!result || !plan || !!failedPlan}
          onChange={onToggle}
        />
      )}
      <label for={id}>
        <span class="head">
          <LaneMark index={lane.index} />
          <b>{lane.reference}</b> <span class="dim">{branch}</span>
          {result && (
            <>
              {' '}
              <span class={result.startsWith('failed') ? 'tone-failure' : ''}>
                <Glyph state={result === 'cleaning' ? 'cleaning' : done ? 'done' : 'failed'} />{' '}
                {result}
              </span>
            </>
          )}
        </span>
        {!plan && <span class="dim">reading what a clean deletes…</span>}
        {failedPlan && (
          <span class="tone-attention">
            {String(failedPlan)
              .split('\n')[0]
              ?.replace(/^error: /, '')}
          </span>
        )}
        {good?.hooks_skipped && (
          <span class="tone-attention">
            ! skips its clean tasks and teardown:{' '}
            {SKIPPED[good.hooks_skipped] || String(good.hooks_skipped).replace(/_/g, ' ')}
          </span>
        )}
        {good?.infra_error && (
          <span class="tone-attention">! containers unknown: {good.infra_error}</span>
        )}
        {deletes.length > 0 && <span class="dim">{deletes.join(' · ')}</span>}
        {steps.length > 0 && <span class="dim">runs {steps.join(' · ')}</span>}
        {running.length > 0 && (
          <span class="tone-attention">
            ! stops {running.map((r) => r.unit.split(':').pop()).join(', ')} started by{' '}
            <Actors actors={[...new Set(running.map((r) => actorLabel(r.actor)))]} />
          </span>
        )}
      </label>
      <span class="size dim">{bytes != null ? size(bytes) : ''}</span>
    </li>
  );
}
