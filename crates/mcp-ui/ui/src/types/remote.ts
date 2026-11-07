// `devlanes remote ls --json` (the daemon's `host.list`) as the board reads it,
// with the field names of crates/proto and crates/transport.

import type { UnitSummary } from './proto';

/** A lane or unit key in a box registry. */
export interface RegistryLaneKey {
  project: string;
  lane: number;
}

/** One lane a box runs. */
export interface RegistryLane {
  key: RegistryLaneKey;
  index: number;
  domain?: string | null;
}

/** One unit a box runs. */
export interface RegistryUnit {
  key: { lane: RegistryLaneKey; kind: string; name: string };
  hidden?: boolean;
  state: string;
  ports?: { name: string; port: number }[];
  hosts?: { host: string; port: number }[];
}

/** A hostname this machine routes to a box. */
export interface RemoteRoute {
  host: string;
  lane?: string;
  unit?: string;
  url?: string | null;
  state?: string;
}

/** A box port this machine mirrors. */
export interface RemotePort {
  port: number;
  lane: string;
  unit: string;
  state?: string | null;
  addresses?: string[];
}

/** A box lane this machine does not mirror, and why. */
export interface SkippedLane {
  lane: string;
  reason?: string | null;
}

/** One linked box. */
export interface RemoteHost {
  host: string;
  state: string;
  registry?: { lanes?: RegistryLane[]; units?: RegistryUnit[] } | null;
  routes?: RemoteRoute[];
  ports?: RemotePort[];
  skipped_lanes?: SkippedLane[];
}

/** The `host.list` document. */
export interface RemoteList {
  items?: RemoteHost[];
}

/**
 * A box lane shaped like a board row. `reference` is `acme@1 @hetzner`, `box`
 * the lane's own reference on the box, `link` the box's link state and
 * `skipped` the `skipped_lanes` entry when this machine does not mirror it.
 */
export interface BoxLane {
  reference: string;
  box: string;
  host: string;
  link: string;
  project: string;
  index: number;
  units: (UnitSummary & { urls: string[] })[];
  state: string;
  skipped: SkippedLane | null;
}
