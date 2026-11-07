/** Stroke icons on a 16 px grid, for the fullscreen views' row buttons and the log filter. */
const ICONS = {
  open: 'M9 2.5h4.5V7M13.5 2.5 7.5 8.5M12 9.5v3a1 1 0 0 1-1 1H3.5a1 1 0 0 1-1-1V5a1 1 0 0 1 1-1h3',
  stop: 'M5 4h6a1 1 0 0 1 1 1v6a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V5a1 1 0 0 1 1-1z',
  start: 'M5 3.5v9l7-4.5z',
  restart: 'M13 8a5 5 0 1 1-1.5-3.6M13 2.5v2.5h-2.5',
  ask: 'M2.5 3.5h11v7h-6l-3 2.5v-2.5h-2z',
  search: 'M7 3a4 4 0 1 1 0 8 4 4 0 0 1 0-8zM10 10l3.5 3.5'
};

/** The names of the icons the card draws. */
export type IconName = keyof typeof ICONS;

/** An inline icon; decorative, so its button carries the label. */
export function Icon({ name }: { name: IconName }) {
  return (
    <svg viewBox="0 0 16 16" aria-hidden="true" focusable="false">
      <path d={ICONS[name]} />
    </svg>
  );
}
