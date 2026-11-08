// `devlanes lane clean --dry-run --json` (the daemon's `lane.clean.plan`).

import type { Actor } from './proto';

/** What cleaning one lane would do. Sizes are null when the daemon does not know them. */
export interface CleanPlan {
  lane: string;
  project?: string;
  index?: number | null;
  branch?: string | null;
  path?: string;
  worktree?: 'present' | 'missing';
  running?: { unit: string; actor?: Actor | null }[];
  tasks?: { name: string; run?: string | null }[];
  teardown?: { name: string; run?: string | null; container?: string | null }[];
  hooks_skipped?: string | null;
  containers?: { name: string; kind: string }[];
  compose_project?: string | null;
  volumes?: { name: string; bytes: number | null }[];
  infra_error?: string | null;
  data_dir?: { path: string; bytes: number | null } | null;
  logs_bytes?: number | null;
  ports?: { start: number; size: number } | null;
  /** The lane's run and task history goes with it. */
  history?: boolean;
  /** The project's last lane: `stop` its shared containers, or why it cannot tell what to stop. */
  shared_infra?: 'stop' | { skipped: string; message?: string | null };
}
