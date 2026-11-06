// Sizes, durations, URLs and command lines as the CLI and the TUI write them.

import type { Actor, RpcError } from '../types/proto';

/** A duration as the TUI writes it: `45s`, `3m`, `2h`, `6d`. */
export function since(ms: number): string {
  const s = Math.max(0, Math.round(ms / 1000));
  if (s < 60) return `${s}s`;
  if (s < 3600) return `${Math.floor(s / 60)}m`;
  if (s < 86400) return `${Math.floor(s / 3600)}h`;
  return `${Math.floor(s / 86400)}d`;
}

/** A byte count in binary units, as `devlanes status` prints memory. */
export function size(bytes: number | null | undefined): string {
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

/** The host and path of a URL, without `https://`. */
export function shortUrl(url: string): string {
  return url.replace(/^https?:\/\//, '').replace(/\/$/, '');
}

/** One actor as a short label: `you` for a person. */
export function actorLabel(actor: Actor | null | undefined): string {
  if (actor?.kind !== 'agent') return 'you';
  return actor.agent_id || 'agent';
}

/** A unit reference such as `acme@2:api`; `unit` may already be one. */
export function unitRef(lane: string, unit: string): string {
  return unit.includes(':') || !lane ? unit : `${lane}:${unit}`;
}

/**
 * The words of an error or reason code from the daemon: its message when it has
 * one, else the code with spaces.
 */
export function reasonText(reason: RpcError | null | undefined): string {
  if (!reason) return '';
  const data = reason.data || {};
  if (data.message || data.title) return data.message || data.title;
  if (reason.code === 'exited') {
    return data.signal
      ? `killed by signal ${data.signal}`
      : `exited with code ${data.code ?? data.exit_code}`;
  }
  if (reason.code === 'ready_timeout') {
    return `ready check ${data.check || ''} did not pass in ${since(data.timeout_ms || 0)}`;
  }
  return String(reason.code || '').replace(/_/g, ' ');
}

/** The words of a command line, for the `$ devlanes ...` line. */
export function commandLine(args: string[] | undefined): string {
  return ['devlanes', ...(args || [])]
    .map((word) => (/[\s"']/.test(word) ? JSON.stringify(word) : word))
    .join(' ');
}

/**
 * The subcommand path of a command line (`start`, `lane clean`, `debug why`),
 * skipping flags, for a card that has to render before the result says so.
 */
export function commandOf(args: string[] | undefined): string {
  const words = (args || []).filter((word) => !word.startsWith('-'));
  const groups = ['lane', 'daemon', 'debug', 'remote', 'env', 'url', 'infra', 'config'];
  const [first = '', second] = words;
  if (groups.includes(first) && second) return `${first} ${second}`;
  return first;
}
