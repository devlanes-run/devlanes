// The parts every view is built from: the card, its title line, a pill, a status shape,
// a line of state, the fold of a long list and the action bar. They read the shared state for what every card
// does the same way: read-only, a stopped daemon, a stopped poll and fullscreen.

import type { ComponentChildren } from 'preact';
import { ask, display } from '../../store/actions';
import {
  canHandOff,
  card,
  daemonStopped,
  isFullscreen,
  pollStopped,
  readOnly
} from '../../store/card';
import { pollNow } from '../../store/poll';
import { clock } from '../../utils/format';
import { powerHint, type Shape as ShapeName, shapeOf, type Tone } from '../../utils/state';
import { Button, LinkButton } from '../button';

/** The card; `dim` while no daemon runs, so the last data reads as old. */
export function Card({ label, children }: { label: string; children: ComponentChildren }) {
  return (
    <section class={`card${daemonStopped() ? ' dim' : ''}`} aria-label={label}>
      {children}
    </section>
  );
}

/** One pill per card, next to the title. A power `state` explains itself on hover. */
export function Pill({
  tone = 'neutral',
  state = '',
  children
}: {
  tone?: Tone;
  state?: string;
  children: ComponentChildren;
}) {
  return (
    <span class={`pill ${tone}`} title={powerHint(state)}>
      {children}
      <Hint state={state} />
    </span>
  );
}

/**
 * What throttled and frozen mean, as text only screen readers get; the element showing
 * the word sets the same text as its `title` for the hover. Nothing for other states.
 */
export function Hint({ state }: { state: string }) {
  const hint = powerHint(state);
  return hint ? <span class="sr-only">. {hint}</span> : null;
}

/** Rows an inline list shows before {@link Fold} sums up the rest. */
export const SHOWN = 15;

/**
 * The last row of a list longer than {@link SHOWN}: what the folded rows hold and
 * "Show all", which grows the card, or "Show fewer" once open. Hosts discourage
 * scrolling inside a card, so the list never gets a scroll area of its own.
 */
export function Fold({
  open,
  text,
  onToggle
}: {
  open: boolean;
  text: string;
  onToggle: () => void;
}) {
  return (
    <li class="row">
      <span class="grow sec">
        {!open && `${text} · `}
        <LinkButton expanded={open} onClick={onToggle}>
          {open ? 'Show fewer' : 'Show all'}
        </LinkButton>
      </span>
    </li>
  );
}

/** The 8 px shape of a state, with the state as its accessible name. */
export function Shape({ state, shape }: { state: string; shape?: ShapeName }) {
  return (
    <span
      class={`st ${shape || shapeOf(state)}`}
      role="img"
      aria-label={state.replace(/_/g, ' ')}
    />
  );
}

/** The props of {@link Head}. */
interface HeadProps {
  title: ComponentChildren;
  sub?: ComponentChildren;
  pill?: ComponentChildren;
  /** Facts for the line under the title, joined with ` · `. */
  meta?: (string | false | 0 | null | undefined)[];
}

/**
 * The title line: title, a secondary name, the pill and, on a read-only server, a
 * caption saying so; in fullscreen also the way back. Under it one line of facts, which
 * ends with "updated 14:02 · Refresh" once polling stopped. While no daemon runs, the
 * pill says that instead.
 */
export function Head({ title, sub, pill, meta = [] }: HeadProps) {
  const facts = meta.filter(Boolean) as string[];
  const stopped = pollStopped() && card.updatedAt > 0;
  return (
    <>
      <div class="hd">
        <span class="t">{title}</span>
        {sub && <span class="sub">{sub}</span>}
        <span class="end">
          {readOnly() && <span class="caption">read-only</span>}
          {daemonStopped() ? <Pill>Daemon stopped</Pill> : pill}
          {isFullscreen() && <Button label="Exit full screen" onClick={() => display('inline')} />}
        </span>
      </div>
      {(facts.length > 0 || stopped) && (
        <p class="meta">
          {facts.join(' · ')}
          {facts.length > 0 && stopped && ' · '}
          {stopped && (
            <>
              updated {clock(card.updatedAt)} · <LinkButton onClick={pollNow}>Refresh</LinkButton>
            </>
          )}
        </p>
      )}
    </>
  );
}

/** One sentence of state: a warning, an empty list, why something failed. */
export function Line({
  tone,
  children
}: {
  tone?: string | undefined;
  children: ComponentChildren;
}) {
  return <p class={`line${tone ? ` ${tone}` : ''}`}>{children}</p>;
}

/**
 * The action bar: an optional summary, then at most two buttons, the primary last.
 * While no daemon runs, it says so and offers to ask the agent instead, since the card
 * never starts one. A failed action's line goes above it.
 */
export function Actions({
  summary,
  warn,
  children
}: {
  summary?: ComponentChildren;
  warn?: boolean;
  children?: ComponentChildren;
}) {
  const note = card.note && <Line tone={card.note.tone}>{card.note.text}</Line>;
  if (daemonStopped()) {
    return (
      <>
        <Line tone="warn">devlanes daemon is not running</Line>
        {canHandOff() && (
          <div class="ft">
            <Button
              label="Ask agent to start it"
              kind="primary"
              onClick={() => ask('The devlanes daemon is not running. Can you start it?')}
            />
          </div>
        )}
      </>
    );
  }
  const buttons = (Array.isArray(children) ? children : [children]).filter(Boolean);
  if (!summary && !buttons.length) return note || null;
  return (
    <>
      {note}
      <div class="ft">
        {summary && <span class={`sum${warn ? ' warn' : ''}`}>{summary}</span>}
        {buttons}
      </div>
    </>
  );
}

/** Rows at their final height while the first data is on its way, so the card does not jump. */
export function Skeleton({ label }: { label: string }) {
  return (
    <section class="card" aria-label={label} aria-busy="true">
      <div class="hd">
        <span class="sk" style={{ width: '96px', height: '14px' }} />
        <span class="sk" style={{ width: '64px' }} />
      </div>
      <ul class="rows">
        {[62, 48, 70].map((width) => (
          <li class="row" key={width}>
            <span class="sk" style={{ width: '8px', height: '8px' }} />
            <span class="sk" style={{ width: `${width}%` }} />
          </li>
        ))}
      </ul>
    </section>
  );
}
