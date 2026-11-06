// @ts-check
// Data helpers shared by the views: states, glyphs and tones as the TUI draws
// them, sizes and times, and the shapes the daemon sends (crates/proto).

/** The TUI's glyph for each state a unit or lane shows. */
const GLYPHS = {
  ready: '●',
  running: '◉',
  queued: '…',
  failed: '✕',
  exited: '■',
  stopped: '·',
  idle: '·',
  done: '✓',
  orphaned: '◆',
  frozen: '❄',
  throttled: 'z',
  connected: '●',
  needs_auth: '!',
  needs_pairing: '!',
};

/** States drawn with the glyph alone: the TUI keeps healthy rows quiet. */
const QUIET = ['ready', 'running', 'stopped', 'idle', 'done', 'frozen', 'throttled'];

/** The braille spinner frames of the TUI. */
export const SPINNER = '⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏';

/**
 * Whether a state is on its way somewhere, so it gets the spinner.
 * @param {string} state
 */
export function isBusy(state) {
  return ['starting', 'stopping', 'cleaning', 'connecting', 'reconnecting'].includes(state);
}

/**
 * The glyph of a state, or null for one that spins.
 * @param {string} state
 * @returns {string | null}
 */
export function glyphOf(state) {
  if (isBusy(state)) return null;
  return GLYPHS[/** @type {keyof GLYPHS} */ (state)] || '·';
}

/**
 * The tone of a state, after `Tone` in crates/tui/src/theme.rs.
 * @param {string} state
 * @returns {'ok' | 'failure' | 'attention' | 'frozen' | 'quiet' | 'busy'}
 */
export function toneOf(state) {
  switch (state) {
    case 'ready':
    case 'running':
    case 'done':
    case 'connected':
      return 'ok';
    case 'failed':
      return 'failure';
    case 'orphaned':
    case 'needs_auth':
    case 'needs_pairing':
      return 'attention';
    case 'frozen':
      return 'frozen';
    case 'starting':
    case 'stopping':
    case 'queued':
    case 'connecting':
    case 'reconnecting':
      return 'busy';
    default:
      return 'quiet';
  }
}

/**
 * Whether a state needs its word next to the glyph.
 * @param {string} state
 */
export function isQuiet(state) {
  return QUIET.includes(state);
}

/**
 * What a unit shows, as `UnitSummary::shown_state` in crates/proto, plus `done`
 * for a task or setup step that exited 0.
 * @param {any} unit a UnitSummary
 * @returns {string}
 */
export function unitState(unit) {
  if (unit.state === 'ready' && unit.power) return unit.power;
  if (unit.state === 'ready' && unit.ready_check === 'started') return 'running';
  if (unit.state === 'exited' && unit.exit_code === 0 && unit.kind === 'task') return 'done';
  return unit.state;
}

/**
 * The role of a unit, falling back to its kind for daemons without roles.
 * @param {any} unit
 * @returns {string}
 */
export function roleOf(unit) {
  return unit.role || unit.kind || 'service';
}

/**
 * The units a person looks at: services and exec units, without hidden ones.
 * @param {any} lane a LaneSummary
 * @returns {any[]}
 */
export function servicesOf(lane) {
  return (lane.units || []).filter(
    (unit) => !unit.hidden && ['service', 'exec', 'external'].includes(roleOf(unit)),
  );
}

/**
 * Services counted as selected: running or on their way, plus failed ones.
 * @param {any} lane
 */
export function activeServices(lane) {
  return servicesOf(lane).filter((unit) => !['idle', 'stopped', 'exited'].includes(unit.state));
}

/**
 * The start phases of a lane (infra, setup, tasks, services) as rows with a
 * state and a detail, derived from its containers and units.
 * @param {any} lane a LaneSummary
 * @returns {{name: string, state: string, detail: string}[]}
 */
export function phasesOf(lane) {
  const rows = [];
  const containers = lane.containers || [];
  if (containers.length) {
    const bad = containers.filter((c) => /unhealthy|exited|oom|missing|removed/.test(c.state || ''));
    const waiting = containers.filter((c) => ['created', 'starting'].includes(c.state));
    const names = (list) => list.map((c) => c.name).join(', ');
    if (bad.length) {
      rows.push({ name: 'infra', state: 'failed', detail: bad.map((c) => `${c.name} ${c.state}`).join(', ') });
    } else if (waiting.length) {
      rows.push({ name: 'infra', state: 'starting', detail: `waiting for ${names(waiting)}` });
    } else {
      rows.push({ name: 'infra', state: 'done', detail: `${names(containers)} up` });
    }
  }
  for (const [name, role] of [['setup', 'setup'], ['tasks', 'task']]) {
    const steps = (lane.units || []).filter((unit) => roleOf(unit) === role);
    if (!steps.length) continue;
    const failed = steps.filter((unit) => unit.state === 'failed');
    const running = steps.find((unit) => isBusy(unit.state) || unit.state === 'queued');
    const done = steps.filter((unit) => unitState(unit) === 'done' || unit.state === 'ready');
    if (failed.length) {
      rows.push({ name, state: 'failed', detail: failed.map((unit) => `${unit.name} exited ${unit.exit_code ?? ''}`.trim()).join(', ') });
    } else if (running) {
      rows.push({ name, state: 'starting', detail: `${running.name} (${done.length + 1} of ${steps.length})` });
    } else if (done.length) {
      rows.push({ name, state: 'done', detail: steps.length === 1 ? steps[0].name : `${steps.length} steps` });
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

/**
 * The state a lane row shows: frozen and throttled come from its power.
 * @param {any} lane
 * @returns {string}
 */
export function laneState(lane) {
  if (lane.power && lane.state === 'running') return lane.power.level;
  return lane.state || 'idle';
}

/**
 * The words next to a lane's glyph: `3/3 ready`, `api failed`, `frozen 22m`.
 * @param {any} lane
 * @param {number} now
 * @returns {string}
 */
export function laneWords(lane, now) {
  const state = laneState(lane);
  const active = activeServices(lane);
  const ready = active.filter((unit) => unit.state === 'ready').length;
  if (state === 'failed') {
    const failed = (lane.units || []).filter((unit) => unit.state === 'failed').map((unit) => unit.name);
    return failed.length ? `${failed.join(', ')} failed` : 'failed';
  }
  if (lane.power && (state === 'frozen' || state === 'throttled')) {
    return `${state} ${since(now - lane.power.since_ms)}`;
  }
  if (state === 'running') return `${ready}/${active.length} ready`;
  return state;
}

/**
 * Who started the units of a lane, one label each: `you` for a person.
 * @param {any[]} units
 * @returns {string[]}
 */
export function actorsOf(units) {
  const labels = units
    .filter((unit) => !['idle', 'stopped'].includes(unit.state))
    .map((unit) => actorLabel(unit.actor));
  return [...new Set(labels)];
}

/**
 * One actor as a short label.
 * @param {any} actor an Actor from crates/proto
 * @returns {string}
 */
export function actorLabel(actor) {
  if (!actor || actor.kind !== 'agent') return 'you';
  return actor.agent_id || 'agent';
}

/**
 * The first URL a browser opens for a lane: a ready service's public URL.
 * @param {any} lane
 * @returns {string | undefined}
 */
export function laneUrl(lane) {
  const unit = servicesOf(lane).find((u) => u.state === 'ready' && u.public_url);
  return unit && unit.public_url;
}

/**
 * The lanes of linked boxes in `remote ls --json` output, shaped like the
 * LaneSummary rows of the board, as `BoxHost::lanes` in crates/tui/src/boxes.rs
 * builds them. `reference` is `acme@1 @hetzner`, `box` the lane's own
 * reference on the box, `link` the box's link state and `skipped` the
 * `skipped_lanes` entry when this machine does not mirror the lane.
 * @param {any} list the `host.list` document
 * @returns {any[]}
 */
export function boxLanes(list) {
  const rows = [];
  for (const host of (list && list.items) || []) {
    const registry = host.registry;
    if (!host.host || !registry) continue;
    for (const info of registry.lanes || []) {
      const box = `${info.key.project}@${info.key.lane}`;
      const own = (registry.units || []).filter((u) => `${u.key.lane.project}@${u.key.lane.lane}` === box);
      const units = own
        .filter((u) => u.key.kind !== 'container')
        .map((u) => {
          const urls = boxUrls(host, box, u);
          return { name: u.key.name, state: u.state, kind: u.key.kind, role: u.key.kind, hidden: u.hidden, urls, public_url: urls[0] };
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
        skipped: (host.skipped_lanes || []).find((s) => s.lane === box) || null,
      });
    }
  }
  return rows.sort((a, b) => a.project.localeCompare(b.project) || a.index - b.index || a.host.localeCompare(b.host));
}

/**
 * What opens a box unit on this machine, as the TUI lists it: the URLs of its
 * hostnames this machine routes, then the loopback address of each port
 * mirrored here, IPv4 when it is bound there.
 * @param {any} host a `host.list` item
 * @param {string} box the lane's reference on the box
 * @param {any} unit a registry unit
 * @returns {string[]}
 */
function boxUrls(host, box, unit) {
  const urls = [];
  for (const name of unit.hosts || []) {
    const route = (host.routes || []).find((r) => r.host.toLowerCase() === name.host.toLowerCase());
    if (route && route.url) urls.push(route.url);
  }
  for (const port of unit.ports || []) {
    const mirrored = (host.ports || []).find((p) => p.port === port.port && p.lane === box && p.unit === unit.key.name);
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
 * @param {any[]} units
 * @param {boolean} containers
 */
function boxLaneState(units, containers) {
  const services = units.filter((u) => u.kind === 'service');
  if (services.some((u) => ['starting', 'queued'].includes(u.state))) return 'starting';
  if (containers || services.some((u) => ['ready', 'running'].includes(u.state))) return 'running';
  if (services.some((u) => u.state === 'failed')) return 'failed';
  return 'idle';
}

/**
 * A unit reference such as `acme@2:api`; `unit` may already be one.
 * @param {string} lane
 * @param {string} unit
 */
export function unitRef(lane, unit) {
  return unit.includes(':') || !lane ? unit : `${lane}:${unit}`;
}

/**
 * A duration as the TUI writes it: `45s`, `3m`, `2h`, `6d`.
 * @param {number} ms
 */
export function since(ms) {
  const s = Math.max(0, Math.round(ms / 1000));
  if (s < 60) return `${s}s`;
  if (s < 3600) return `${Math.floor(s / 60)}m`;
  if (s < 86400) return `${Math.floor(s / 3600)}h`;
  return `${Math.floor(s / 86400)}d`;
}

/**
 * A byte count in binary units, as `devlanes status` prints memory.
 * @param {number | null | undefined} bytes
 */
export function size(bytes) {
  if (bytes == null) return '';
  const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB'];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit++;
  }
  return `${unit && value < 10 ? value.toFixed(1) : Math.round(value)} ${units[unit]}`;
}

/**
 * The host and path of a URL, without `https://`.
 * @param {string} url
 */
export function shortUrl(url) {
  return url.replace(/^https?:\/\//, '').replace(/\/$/, '');
}

/**
 * The port a unit listens on, first of its ports.
 * @param {any} unit
 * @returns {number | undefined}
 */
export function portOf(unit) {
  const ports = Object.values(unit.ports || {});
  return ports.length ? Number(ports[0]) : undefined;
}

/**
 * The CPU history of a unit or lane from `debug top --json` data, for a sparkline.
 * @param {any} top
 * @param {string} lane
 * @param {string} [unit]
 * @returns {number[]}
 */
export function cpuHistory(top, lane, unit) {
  if (!top) return [];
  const rows = unit ? top.units : top.lanes;
  const row = (rows || []).find((r) => r.lane === lane && (!unit || r.unit === unit) && !r.ended);
  return row ? (row.history || []).map((point) => point[1] || 0) : [];
}

/**
 * The words of an error or reason code from the daemon: its message when it has
 * one, else the code with spaces.
 * @param {any} reason an RpcError or a `why` reason, `{code, data}`
 */
export function reasonText(reason) {
  if (!reason) return '';
  const data = reason.data || {};
  if (data.message || data.title) return data.message || data.title;
  if (reason.code === 'exited') return data.signal ? `killed by signal ${data.signal}` : `exited with code ${data.code ?? data.exit_code}`;
  if (reason.code === 'ready_timeout') return `ready check ${data.check || ''} did not pass in ${since(data.timeout_ms || 0)}`;
  return String(reason.code || '').replace(/_/g, ' ');
}

/**
 * The words of a command line, for the `$ devlanes ...` line.
 * @param {string[]} args
 */
export function commandLine(args) {
  return ['devlanes', ...(args || [])].map((word) => (/[\s"']/.test(word) ? JSON.stringify(word) : word)).join(' ');
}

/**
 * The subcommand path of a command line (`start`, `lane clean`, `debug why`),
 * skipping flags, for a card that has to render before the result says so.
 * @param {string[]} args
 */
export function commandOf(args) {
  const words = (args || []).filter((word) => !word.startsWith('-'));
  const groups = ['lane', 'daemon', 'debug', 'remote', 'env', 'url', 'infra', 'config'];
  if (groups.includes(words[0]) && words[1]) return `${words[0]} ${words[1]}`;
  return words[0] || '';
}
