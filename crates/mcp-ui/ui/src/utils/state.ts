// States as the card shows them: a shape, a tone and a word.

import type { UnitSummary } from '../types/proto';

/** The status shapes the stylesheet draws: dot, ring, spinning arc, diamond. */
export type Shape = 'ok' | 'info' | 'warn' | 'busy' | 'fail' | 'idle';

/** The tones of pills and words, after the standard `--color-text-<tone>` names. */
export type Tone = 'success' | 'danger' | 'warning' | 'info' | 'neutral';

/** Whether a state is on its way somewhere, so it spins. */
export function isBusy(state: string): boolean {
  return ['starting', 'stopping', 'cleaning', 'connecting', 'reconnecting', 'queued'].includes(
    state
  );
}

/** The shape of a state. Failed gets the diamond: in Claude's light theme colour alone is too close to the text. */
export function shapeOf(state: string): Shape {
  if (isBusy(state)) return 'busy';
  switch (state) {
    case 'ready':
    case 'running':
    case 'done':
    case 'connected':
      return 'ok';
    case 'failed':
      return 'fail';
    case 'frozen':
    case 'throttled':
      return 'info';
    case 'orphaned':
    case 'needs_auth':
    case 'needs_pairing':
      return 'warn';
    default:
      return 'idle';
  }
}

/** The tone of a state's word. Healthy states stay quiet. */
export function toneOf(state: string): Tone {
  switch (shapeOf(state)) {
    case 'fail':
      return 'danger';
    case 'info':
      return 'info';
    case 'warn':
      return 'warning';
    default:
      return 'neutral';
  }
}

/** The class that colours a state's word in a row. */
export function wordClass(state: string): string {
  const tone = toneOf(state);
  if (tone === 'neutral') return shapeOf(state) === 'idle' ? 'ter' : 'sec';
  return tone === 'warning' ? 'warn' : tone;
}

/** A state code as words: `needs_pairing` is `needs pairing`. */
export function words(state: string): string {
  return String(state || '').replace(/_/g, ' ');
}

/** A word with a capital first letter, for pills. */
export function capital(text: string): string {
  return text.charAt(0).toUpperCase() + text.slice(1);
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
