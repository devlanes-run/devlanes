// Lanes and units as the cards show them: phases, words, URLs, CPU history and
// the lanes of linked boxes, from the shapes the daemon sends (crates/proto).

import type { LaneSummary, TopPage, UnitSummary } from '../types/proto';
import type { BoxLane, RegistryUnit, RemoteHost, RemoteList } from '../types/remote';
import { actorLabel, since } from './format';
import { isBusy, unitState } from './state';

/** One start phase row of the lane card. */
export interface Phase {
  name: string;
  state: string;
  detail: string;
}

/** The fields the helpers below read, shared by local and box lanes. */
type LaneLike = Pick<LaneSummary, 'state' | 'units'> & Partial<Pick<LaneSummary, 'power'>>;

/** The role of a unit, falling back to its kind for daemons without roles. */
export function roleOf(unit: UnitSummary): string {
  return unit.role || unit.kind || 'service';
}

/** The units a person looks at: services and exec units, without hidden ones. */
export function servicesOf(lane: Pick<LaneSummary, 'units'>): UnitSummary[] {
  return (lane.units || []).filter(
    (unit) => !unit.hidden && ['service', 'exec', 'external'].includes(roleOf(unit))
  );
}

/** Services counted as selected: running or on their way, plus failed ones. */
export function activeServices(lane: Pick<LaneSummary, 'units'>): UnitSummary[] {
  return servicesOf(lane).filter((unit) => !['idle', 'stopped', 'exited'].includes(unit.state));
}

/**
 * The start phases of a lane (infra, setup, tasks, services) as rows with a
 * state and a detail, derived from its containers and units.
 */
export function phasesOf(lane: LaneSummary): Phase[] {
  const rows: Phase[] = [];
  const containers = lane.containers || [];
  if (containers.length) {
    const bad = containers.filter((c) =>
      /unhealthy|exited|oom|missing|removed/.test(c.state || '')
    );
    const waiting = containers.filter((c) => ['created', 'starting'].includes(c.state || ''));
    const names = (list: typeof containers) => list.map((c) => c.name).join(', ');
    if (bad.length) {
      rows.push({
        name: 'infra',
        state: 'failed',
        detail: bad.map((c) => `${c.name} ${c.state}`).join(', ')
      });
    } else if (waiting.length) {
      rows.push({ name: 'infra', state: 'starting', detail: `waiting for ${names(waiting)}` });
    } else {
      rows.push({ name: 'infra', state: 'done', detail: `${names(containers)} up` });
    }
  }
  for (const [name, role] of [
    ['setup', 'setup'],
    ['tasks', 'task']
  ] as const) {
    const steps = (lane.units || []).filter((unit) => roleOf(unit) === role);
    const [only] = steps;
    if (!only) continue;
    const failed = steps.filter((unit) => unit.state === 'failed');
    const running = steps.find((unit) => isBusy(unit.state) || unit.state === 'queued');
    const done = steps.filter((unit) => unitState(unit) === 'done' || unit.state === 'ready');
    if (failed.length) {
      rows.push({
        name,
        state: 'failed',
        detail: failed
          .map((unit) => `${unit.name} exited ${unit.exit_code ?? ''}`.trim())
          .join(', ')
      });
    } else if (running) {
      rows.push({
        name,
        state: 'starting',
        detail: `${running.name} (${done.length + 1} of ${steps.length})`
      });
    } else if (done.length) {
      rows.push({
        name,
        state: 'done',
        detail: steps.length === 1 ? only.name : `${steps.length} steps`
      });
    }
  }
  const active = activeServices(lane);
  if (active.length) {
    const ready = active.filter((unit) => unit.state === 'ready').length;
    const failed = active.filter((unit) => unit.state === 'failed');
    const state = failed.length ? 'failed' : ready === active.length ? 'done' : 'starting';
    const detail = failed.length
      ? `${failed.map((unit) => unit.name).join(', ')} failed`
      : `${ready} of ${active.length} ready`;
    rows.push({ name: 'services', state, detail });
  }
  return rows;
}

/** The state a lane row shows: frozen and throttled come from its power. */
export function laneState(lane: LaneLike): string {
  if (lane.power && lane.state === 'running') return lane.power.level;
  return lane.state || 'idle';
}

/** The words next to a lane's glyph: `3/3 ready`, `api failed`, `frozen 22m`. */
export function laneWords(lane: LaneLike, now: number): string {
  const state = laneState(lane);
  const active = activeServices(lane);
  const ready = active.filter((unit) => unit.state === 'ready').length;
  if (state === 'failed') {
    const failed = (lane.units || [])
      .filter((unit) => unit.state === 'failed')
      .map((unit) => unit.name);
    return failed.length ? `${failed.join(', ')} failed` : 'failed';
  }
  if (lane.power && (state === 'frozen' || state === 'throttled')) {
    return `${state} ${since(now - lane.power.since_ms)}`;
  }
  if (state === 'running') return `${ready}/${active.length} ready`;
  return state;
}

/** Who started the running units of a lane, one label each. */
export function actorsOf(units: UnitSummary[]): string[] {
  const labels = units
    .filter((unit) => !['idle', 'stopped'].includes(unit.state))
    .map((unit) => actorLabel(unit.actor));
  return [...new Set(labels)];
}

/** The first URL a browser opens for a lane: a ready service's public URL. */
export function laneUrl(lane: Pick<LaneSummary, 'units'>): string | undefined {
  const unit = servicesOf(lane).find((u) => u.state === 'ready' && u.public_url);
  return unit?.public_url ?? undefined;
}

/** The port a unit listens on, first of its ports. */
export function portOf(unit: UnitSummary): number | undefined {
  const ports = Object.values(unit.ports || {});
  return ports.length ? Number(ports[0]) : undefined;
}

/** The CPU history of a unit or lane from `debug top --json` data, for a sparkline. */
export function cpuHistory(top: TopPage | null | undefined, lane: string, unit?: string): number[] {
  if (!top) return [];
  const rows = unit ? top.units : top.lanes;
  const row = (rows || []).find((r) => r.lane === lane && (!unit || r.unit === unit) && !r.ended);
  return row ? (row.history || []).map((point) => point[1] || 0) : [];
}

/**
 * The lanes of linked boxes in `remote ls --json` output, shaped like the
 * LaneSummary rows of the board, as `BoxHost::lanes` in crates/tui/src/boxes.rs
 * builds them.
 */
export function boxLanes(list: RemoteList | null | undefined): BoxLane[] {
  const rows: BoxLane[] = [];
  for (const host of list?.items || []) {
    const registry = host.registry;
    if (!host.host || !registry) continue;
    for (const info of registry.lanes || []) {
      const box = `${info.key.project}@${info.key.lane}`;
      const own = (registry.units || []).filter(
        (u) => `${u.key.lane.project}@${u.key.lane.lane}` === box
      );
      const units = own
        .filter((u) => u.key.kind !== 'container')
        .map((u) => {
          const urls = boxUrls(host, box, u);
          return {
            name: u.key.name,
            state: u.state,
            kind: u.key.kind,
            role: u.key.kind,
            hidden: u.hidden ?? false,
            urls,
            public_url: urls[0] ?? null
          };
        });
      const containers = own.filter((u) => u.key.kind === 'container' && !u.hidden);
      rows.push({
        reference: `${box} @${host.host}`,
        box,
        host: host.host,
        link: host.state,
        project: info.key.project,
        index: info.index,
        units,
        state: boxLaneState(units, containers.length > 0),
        skipped: (host.skipped_lanes || []).find((s) => s.lane === box) || null
      });
    }
  }
  return rows.sort(
    (a, b) =>
      a.project.localeCompare(b.project) || a.index - b.index || a.host.localeCompare(b.host)
  );
}

/**
 * What opens a box unit on this machine, as the TUI lists it: the URLs of its
 * hostnames this machine routes, then the loopback address of each port
 * mirrored here, IPv4 when it is bound there.
 */
function boxUrls(host: RemoteHost, box: string, unit: RegistryUnit): string[] {
  const urls: string[] = [];
  for (const name of unit.hosts || []) {
    const route = (host.routes || []).find((r) => r.host.toLowerCase() === name.host.toLowerCase());
    if (route?.url) urls.push(route.url);
  }
  for (const port of unit.ports || []) {
    const mirrored = (host.ports || []).find(
      (p) => p.port === port.port && p.lane === box && p.unit === unit.key.name
    );
    if (!mirrored || (mirrored.state || 'bound') !== 'bound') continue;
    const addresses = mirrored.addresses || [];
    if (addresses.includes('127.0.0.1')) urls.push(`http://127.0.0.1:${port.port}`);
    else if (addresses.includes('::1')) urls.push(`http://[::1]:${port.port}`);
  }
  return [...new Set(urls)];
}

/**
 * A box lane's state from its services, as the TUI derives it: starting while
 * one starts, running while one is up or a container runs, then failed, else idle.
 */
function boxLaneState(units: UnitSummary[], containers: boolean): string {
  const services = units.filter((u) => u.kind === 'service');
  if (services.some((u) => ['starting', 'queued'].includes(u.state))) return 'starting';
  if (containers || services.some((u) => ['ready', 'running'].includes(u.state))) return 'running';
  if (services.some((u) => u.state === 'failed')) return 'failed';
  return 'idle';
}
