// @ts-check
// Shared pieces of every card: the terminal window, buttons, icons, glyphs and
// sparklines.
import { html, useEffect, useState } from '../vendor/standalone.module.js';
import { SPINNER, glyphOf, toneOf } from './util.js';

/** Material Design Icons paths (Apache-2.0, pictogrammers.com), 24 by 24. */
const ICONS = {
  open: 'M14,3V5H17.59L7.76,14.83L9.17,16.24L19,6.41V10H21V3M19,19H5V5H12V3H5C3.89,3 3,3.9 3,5V19A2,2 0 0,0 5,21H19A2,2 0 0,0 21,19V12H19V19Z',
  restart: 'M12,4C14.1,4 16.1,4.8 17.6,6.3C20.7,9.4 20.7,14.5 17.6,17.6C15.8,19.5 13.3,20.2 10.9,19.9L11.4,17.9C13.1,18.1 14.9,17.5 16.2,16.2C18.5,13.9 18.5,10.1 16.2,7.7C15.1,6.6 13.5,6 12,6V10.6L7,5.6L12,0.6V4M6.3,17.6C3.7,15 3.3,11 5.1,7.9L6.6,9.4C5.5,11.6 5.9,14.4 7.8,16.2C8.3,16.7 8.9,17.1 9.6,17.4L9,19.4C8,19 7.1,18.4 6.3,17.6Z',
  stop: 'M18,18H6V6H18V18Z',
  start: 'M8,5.14V19.14L19,12.14L8,5.14Z',
  fullscreen: 'M5,5H10V7H7V10H5V5M14,5H19V10H17V7H14V5M17,14H19V19H14V17H17V14M10,17V19H5V14H7V17H10Z',
  inline: 'M14,14H19V16H16V19H14V14M5,14H10V19H8V16H5V14M8,5H10V10H5V8H8V5M19,8V10H14V5H16V8H19Z',
  copy: 'M19,21H8V7H19M19,5H8A2,2 0 0,0 6,7V21A2,2 0 0,0 8,23H19A2,2 0 0,0 21,21V7A2,2 0 0,0 19,5M16,1H4A2,2 0 0,0 2,3V17H4V3H16V1Z',
  ask: 'M20 2H4C2.9 2 2 2.9 2 4V22L6 18H20C21.1 18 22 17.1 22 16V4C22 2.9 21.1 2 20 2M20 16H5.2L4 17.2V4H20V16Z',
  back: 'M20,11V13H8L13.5,18.5L12.08,19.92L4.16,12L12.08,4.08L13.5,5.5L8,11H20Z',
  grep: 'M9.5,3A6.5,6.5 0 0,1 16,9.5C16,11.11 15.41,12.59 14.44,13.73L14.71,14H15.5L20.5,19L19,20.5L14,15.5V14.71L13.73,14.44C12.59,15.41 11.11,16 9.5,16A6.5,6.5 0 0,1 3,9.5A6.5,6.5 0 0,1 9.5,3M9.5,5C7,5 5,7 5,9.5C5,12 7,14 9.5,14C12,14 14,12 14,9.5C14,7 12,5 9.5,5Z',
};

/** Whether the person asked for less motion; the spinner then stands still. */
const REDUCED_MOTION = window.matchMedia('(prefers-reduced-motion: reduce)').matches;

/**
 * An inline icon; decorative, so buttons carry the label.
 * @param {{name: keyof ICONS}} props
 */
export function Icon({ name }) {
  return html`<svg class="icon" viewBox="0 0 24 24" aria-hidden="true" focusable="false"><path d=${ICONS[name]} /></svg>`;
}

/** The devlanes mark: three lanes, the first one painted. */
export function Mark() {
  return html`<svg class="mark" viewBox="0 0 64 64" aria-hidden="true" focusable="false"><rect x="8" y="16" width="32" height="8" rx="4" fill="#f5c542" /><rect x="16" y="28" width="32" height="8" rx="4" fill="currentColor" /><rect x="24" y="40" width="32" height="8" rx="4" fill="currentColor" /></svg>`;
}

/**
 * A compact site button. With `iconOnly` the label becomes its accessible name
 * and tooltip.
 * @param {{icon?: keyof ICONS, label: string, onClick?: () => void, primary?: boolean, disabled?: boolean, iconOnly?: boolean, cls?: string}} props
 */
export function Button({ icon, label, onClick, primary, disabled, iconOnly, cls }) {
  const classes = `btn${primary ? ' primary' : ''}${iconOnly ? ' icon-only' : ''}${cls ? ` ${cls}` : ''}`;
  return html`<button type="button" class=${classes} onClick=${onClick} disabled=${disabled} aria-label=${iconOnly ? label : undefined} title=${iconOnly ? label : undefined}>${icon && html`<${Icon} name=${icon} />`}${!iconOnly && html`<span>${label}</span>`}</button>`;
}

/**
 * The braille spinner of the TUI, one frame every 100 ms.
 * @param {{label?: string}} props
 */
export function Spinner({ label }) {
  const [frame, setFrame] = useState(0);
  useEffect(() => {
    if (REDUCED_MOTION) return undefined;
    const timer = setInterval(() => setFrame((f) => (f + 1) % SPINNER.length), 100);
    return () => clearInterval(timer);
  }, []);
  return html`<span class="glyph tone-busy" role=${label ? 'img' : undefined} aria-label=${label} aria-hidden=${label ? undefined : 'true'}>${SPINNER[frame]}</span>`;
}

/**
 * A state's glyph in its tone; the spinner for busy states.
 * @param {{state: string}} props
 */
export function Glyph({ state }) {
  const glyph = glyphOf(state);
  if (glyph === null) return html`<${Spinner} />`;
  return html`<span class=${`glyph tone-${toneOf(state)}`} aria-hidden="true">${glyph}</span>`;
}

/**
 * A CPU sparkline, scaled to its own peak.
 * @param {{values: number[], label?: string}} props
 */
export function Spark({ values, label }) {
  if (values.length < 2) return null;
  const width = 56;
  const height = 14;
  const max = Math.max(...values, 1);
  const points = values
    .map((v, i) => `${((i / (values.length - 1)) * width).toFixed(1)},${(height - 1 - (v / max) * (height - 2)).toFixed(1)}`)
    .join(' ');
  const last = values[values.length - 1];
  return html`<svg class="spark" width=${width} height=${height} viewBox=${`0 0 ${width} ${height}`} role="img" aria-label=${`${label || 'cpu'} ${last.toFixed(1)}%`}><polyline points=${points} /></svg>`;
}

/**
 * The website's terminal window: dots, a centred title, tools on the right,
 * the body and an optional footer.
 * @param {{title: any, label: string, tools?: any, foot?: any, children?: any}} props
 */
export function Window({ title, label, tools, foot, children }) {
  return html`<section class="term" aria-label=${label}>
    <header class="term-bar">
      <span class="dots" aria-hidden="true"><i></i><i></i><i></i></span>
      <span class="term-title"><${Mark} /><span>${title}</span></span>
      <span class="term-tools">${tools}</span>
    </header>
    <div class="term-body">${children}</div>
    ${foot && html`<footer class="term-foot">${foot}</footer>`}
  </section>`;
}

/**
 * A notched pane, as the TUI draws its boxes: a border with the title in it.
 * @param {{title: any, children?: any, cls?: string}} props
 */
export function Pane({ title, children, cls }) {
  return html`<section class=${`pane ${cls || ''}`}><h2 class="pane-title">${title}</h2>${children}</section>`;
}

/**
 * The lane colour mark of the website's lane table.
 * @param {{index: number | null | undefined}} props
 */
export function LaneMark({ index }) {
  return html`<span class=${`lane-mark lane-${(index ?? 0) % 4}`} aria-hidden="true"></span>`;
}

/**
 * Actor badges: who started the units.
 * @param {{actors: string[]}} props
 */
export function Actors({ actors }) {
  return actors.map((actor) => html`<span class=${`actor${actor === 'you' ? ' you' : ''}`} title=${actor}>${actor}</span>`);
}

/**
 * A one-line note under a card: an error, or what a button did.
 * @param {{note: {text: string, tone?: string} | null}} props
 */
export function Note({ note }) {
  if (!note) return null;
  return html`<p class=${`note tone-${note.tone || 'quiet'}`} role="status">${note.text}</p>`;
}
