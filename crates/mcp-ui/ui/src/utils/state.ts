// States, glyphs and tones as the TUI draws them (crates/tui/src/theme.rs).

import type { UnitSummary } from '../types/proto';

/** The TUI's glyph for each state a unit or lane shows. */
const GLYPHS: Record<string, string> = {
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
  needs_pairing: '!'
};

/** States drawn with the glyph alone: the TUI keeps healthy rows quiet. */
const QUIET = ['ready', 'running', 'stopped', 'idle', 'done', 'frozen', 'throttled'];

/** The braille spinner frames of the TUI. */
export const SPINNER = '⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏';

/** The tone classes the stylesheet paints. */
export type Tone = 'ok' | 'failure' | 'attention' | 'frozen' | 'quiet' | 'busy';

/** Whether a state is on its way somewhere, so it gets the spinner. */
export function isBusy(state: string): boolean {
  return ['starting', 'stopping', 'cleaning', 'connecting', 'reconnecting'].includes(state);
}

/** The glyph of a state, or null for one that spins. */
export function glyphOf(state: string): string | null {
  if (isBusy(state)) return null;
  return GLYPHS[state] || '·';
}

/** The tone of a state, after `Tone` in crates/tui/src/theme.rs. */
export function toneOf(state: string): Tone {
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

/** Whether a state goes without its word next to the glyph. */
export function isQuiet(state: string): boolean {
  return QUIET.includes(state);
}

/**
 * What a unit shows, as `UnitSummary::shown_state` in crates/proto, plus `done`
 * for a task or setup step that exited 0.
 */
export function unitState(unit: UnitSummary): string {
  if (unit.state === 'ready' && unit.power) return unit.power;
  if (unit.state === 'ready' && unit.ready_check === 'started') return 'running';
  if (unit.state === 'exited' && unit.exit_code === 0 && unit.kind === 'task') return 'done';
  return unit.state;
}
