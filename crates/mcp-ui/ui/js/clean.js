// @ts-check
// The clean planner: the project's lanes, what cleaning each one deletes (from
// `lane clean --dry-run`), checkboxes and a confirm inside the card. The click
// on the confirm is the consent, so `clean` runs with --yes on the server.
import { html, useEffect, useState } from '../vendor/standalone.module.js';
import { Actors, Button, Glyph, LaneMark, Note, Window } from './ui.js';
import { FullscreenButton, act, ask, bridge, card, cwd, readOnly, structured, tellModel } from './store.js';
import { actorLabel, size } from './util.js';

/** Why a clean would skip a lane's tasks and teardown, as `lane clean --dry-run` says it. */
const SKIPPED = {
  worktree_missing: 'the worktree is gone',
  needs_env: 'the lane needs a start from a shell first',
  unassigned: 'the worktree has no lane',
};

/**
 * The bytes a plan frees, or null when the daemon knows none of its sizes.
 * @param {any} plan the `lane clean --dry-run --json` output
 * @returns {number | null}
 */
function planBytes(plan) {
  const sizes = [...(plan.volumes || []).map((v) => v.bytes), plan.data_dir && plan.data_dir.bytes, plan.logs_bytes];
  const known = sizes.filter((n) => typeof n === 'number');
  return known.length ? known.reduce((a, b) => a + b, 0) : null;
}

/** The clean planner. */
export function CleanView() {
  const sc = structured() || {};
  const args = sc.args || [];
  const dryRun = args.includes('--dry-run');
  const target = (sc.data && sc.data.lane) || args.filter((a) => !a.startsWith('-') && a !== 'lane' && a !== 'clean')[0];
  const [lanes, setLanes] = useState(/** @type {any[] | null} */ (null));
  const [plans, setPlans] = useState(/** @type {Record<string, any>} */ (dryRun && sc.data ? { [sc.data.lane]: sc.data } : {}));
  const [checked, setChecked] = useState(/** @type {string[]} */ (target && dryRun ? [target] : []));
  const [results, setResults] = useState(/** @type {Record<string, string>} */ (!dryRun && target && !sc.exit_code ? { [target]: 'cleaned' } : {}));
  const [confirm, setConfirm] = useState(false);
  const [error, setError] = useState('');
  // Read-only servers offer no app_action, so the card shows only the plan the result holds.
  const ro = readOnly();

  useEffect(() => {
    if (ro) return undefined;
    let gone = false;
    (async () => {
      try {
        const snap = (await bridge.callTool('app_snapshot', { cwd: cwd() })).structuredContent || {};
        if (snap.daemon === 'stopped') throw new Error('the daemon is not running');
        const items = ((snap.status && snap.status.items) || [])
          .slice()
          .sort((a, b) => (a.index ?? 0) - (b.index ?? 0));
        if (gone) return;
        setLanes(items);
        // One plan at a time: each one sizes volumes and folders on disk.
        for (const lane of items) {
          if (gone || plans[lane.reference] || results[lane.reference]) continue;
          const out = await bridge
            .callTool('app_action', { cwd: cwd(), action: 'clean_plan', target: lane.reference })
            .then((result) => result.structuredContent || {}, (e) => ({ exit_code: 1, text: e.message }));
          if (gone) return;
          setPlans((old) => ({ ...old, [lane.reference]: !out.exit_code && out.data ? out.data : { error: out.text || 'no plan' } }));
        }
      } catch (e) {
        if (!gone) setError(/** @type {Error} */ (e).message);
      }
    })();
    return () => {
      gone = true;
    };
  }, []);

  const rows = lanes || (target ? [{ reference: target, index: plans[target] && plans[target].index, branch: plans[target] && plans[target].branch }] : []);
  const chosen = checked.filter((ref) => !results[ref]);
  const total = chosen.reduce((sum, ref) => sum + ((plans[ref] && planBytes(plans[ref])) || 0), 0);
  const toggle = (ref) => setChecked((old) => (old.includes(ref) ? old.filter((r) => r !== ref) : [...old, ref]));

  const clean = async () => {
    setConfirm(false);
    const done = [];
    for (const ref of chosen) {
      setResults((old) => ({ ...old, [ref]: 'cleaning' }));
      const out = await act('clean', ref);
      const ok = out && !out.exit_code;
      setResults((old) => ({ ...old, [ref]: ok ? 'cleaned' : `failed: ${String((out && out.text) || '').split('\n')[0]}` }));
      if (ok) done.push(ref);
    }
    if (done.length) tellModel(`user cleaned ${done.join(', ')} from the card`);
  };

  const foot = html`<span class="poll">${chosen.length ? `frees about ${size(total) || '0 B'}` : 'pick the lanes to clean'}</span>
    <span class="actions">
      <${Button} icon="ask" label="ask agent first" onClick=${() => ask(chosen.length ? `Before I clean ${chosen.join(', ')}: is anything there not pushed or not merged?` : 'Which of my lanes are safe to clean?')} />
      ${!ro && html`<${Button} label=${`clean ${chosen.length || ''} ${chosen.length === 1 ? 'lane' : 'lanes'}`.replace('  ', ' ')} primary disabled=${!chosen.length || confirm} onClick=${() => setConfirm(true)} />`}
    </span>`;

  return html`<${Window} title=${html`<b>clean lanes</b>${rows[0] && rows[0].project && html` <span class="dim">· ${rows[0].project}</span>`}`} label="clean planner" tools=${html`<${FullscreenButton} />`} foot=${foot}>
    ${error && html`<p class="note tone-failure" role="status">${error}</p>`}
    ${rows.length === 0 && !error && html`<p class="dim">loading lanes…</p>`}
    <ul class="plans">${rows.map((lane) => html`<${PlanRow} lane=${lane} plan=${plans[lane.reference]} result=${results[lane.reference]} checked=${checked.includes(lane.reference)} ro=${ro} onToggle=${() => toggle(lane.reference)} />`)}</ul>
    ${confirm && html`<div class="banner confirm" role="alertdialog" aria-label="confirm clean">
      <p><b>clean ${chosen.join(', ')}?</b></p>
      <p>Runs their clean tasks and teardown, deletes the volumes and data folders listed above and frees their ports. This can't be undone.</p>
      <p class="actions"><${Button} label="cancel" onClick=${() => setConfirm(false)} /><${Button} label=${`clean ${chosen.length}`} primary onClick=${clean} /></p>
    </div>`}
    <${Note} note=${card.note} />
  <//>`;
}

/**
 * One lane with what cleaning it deletes; without a checkbox when `ro`.
 * @param {{lane: any, plan: any, result?: string, checked: boolean, ro: boolean, onToggle: () => void}} props
 */
function PlanRow({ lane, plan, result, checked, ro, onToggle }) {
  const id = `clean-${lane.reference.replace(/[^\w-]/g, '-')}`;
  const bytes = plan && !plan.error ? planBytes(plan) : null;
  const deletes = [];
  if (plan && !plan.error) {
    for (const v of plan.volumes || []) deletes.push(`volume ${v.name}${v.bytes != null ? ` ${size(v.bytes)}` : ''}`);
    if (plan.containers && plan.containers.length) deletes.push(`${plan.containers.length} ${plan.containers.length === 1 ? 'container' : 'containers'}`);
    if (plan.data_dir) deletes.push(`data folder${plan.data_dir.bytes != null ? ` ${size(plan.data_dir.bytes)}` : ''}`);
    if (plan.logs_bytes) deletes.push(`logs ${size(plan.logs_bytes)}`);
  }
  const steps = plan && !plan.error ? [...(plan.teardown || []).map((t) => `teardown ${t.name}`), ...(plan.tasks || []).map((t) => `task ${t.name}`)] : [];
  const running = (plan && plan.running) || [];
  const branch = lane.name || lane.branch || (plan && plan.branch) || '';
  const done = result === 'cleaned';
  return html`<li class=${`plan${done ? ' done' : ''}${ro ? ' ro' : ''}`}>
    ${!ro && html`<input type="checkbox" id=${id} checked=${checked && !done} disabled=${!!result || !plan || !!(plan && plan.error)} onChange=${onToggle} />`}
    <label for=${id}>
      <span class="head"><${LaneMark} index=${lane.index} /><b>${lane.reference}</b> <span class="dim">${branch}</span>
        ${result && html` <span class=${result.startsWith('failed') ? 'tone-failure' : ''}><${Glyph} state=${result === 'cleaning' ? 'cleaning' : done ? 'done' : 'failed'} /> ${result}</span>`}</span>
      ${!plan && html`<span class="dim">reading what a clean deletes…</span>`}
      ${plan && plan.error && html`<span class="tone-attention">${String(plan.error).split('\n')[0].replace(/^error: /, '')}</span>`}
      ${plan && plan.hooks_skipped && html`<span class="tone-attention">! skips its clean tasks and teardown: ${SKIPPED[plan.hooks_skipped] || String(plan.hooks_skipped).replace(/_/g, ' ')}</span>`}
      ${plan && plan.infra_error && html`<span class="tone-attention">! containers unknown: ${plan.infra_error}</span>`}
      ${deletes.length > 0 && html`<span class="dim">${deletes.join(' · ')}</span>`}
      ${steps.length > 0 && html`<span class="dim">runs ${steps.join(' · ')}</span>`}
      ${running.length > 0 && html`<span class="tone-attention">! stops ${running.map((r) => r.unit.split(':').pop()).join(', ')} started by <${Actors} actors=${[...new Set(running.map((r) => actorLabel(r.actor)))]} /></span>`}
    </label>
    <span class="size dim">${bytes != null ? size(bytes) : ''}</span>
  </li>`;
}
