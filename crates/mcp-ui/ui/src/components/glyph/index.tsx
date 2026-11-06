import { useEffect, useState } from 'preact/hooks';
import { glyphOf, SPINNER, toneOf } from '../../utils/state';

/** Whether the person asked for less motion; the spinner then stands still. */
const REDUCED_MOTION = window.matchMedia('(prefers-reduced-motion: reduce)').matches;

/** The braille spinner of the TUI, one frame every 100 ms. */
export function Spinner({ label }: { label?: string }) {
  const [frame, setFrame] = useState(0);
  useEffect(() => {
    if (REDUCED_MOTION) return undefined;
    const timer = setInterval(() => setFrame((f) => (f + 1) % SPINNER.length), 100);
    return () => clearInterval(timer);
  }, []);
  if (!label) {
    return (
      <span class="glyph tone-busy" aria-hidden="true">
        {SPINNER[frame]}
      </span>
    );
  }
  return (
    <span class="glyph tone-busy" role="img" aria-label={label}>
      {SPINNER[frame]}
    </span>
  );
}

/** A state's glyph in its tone; the spinner for busy states. */
export function Glyph({ state }: { state: string }) {
  const glyph = glyphOf(state);
  if (glyph === null) return <Spinner />;
  return (
    <span class={`glyph tone-${toneOf(state)}`} aria-hidden="true">
      {glyph}
    </span>
  );
}
