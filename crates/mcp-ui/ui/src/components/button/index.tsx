import type { ComponentChildren } from 'preact';
import { Icon, type IconName } from '../icon';

/** The props of {@link Button}. */
export interface ButtonProps {
  label: string;
  onClick?: () => void;
  /** `primary` is the next step (one per card), `danger` and `destroy` delete things. */
  kind?: 'primary' | 'danger' | 'destroy';
  disabled?: boolean;
  /** Why the button is disabled, or what it acts on. */
  title?: string;
}

/** A labelled button of the action bar. */
export function Button({ label, onClick, kind, disabled, title }: ButtonProps) {
  return (
    <button
      type="button"
      class={`btn${kind ? ` ${kind}` : ''}`}
      onClick={onClick}
      disabled={disabled}
      title={title}
    >
      {label}
    </button>
  );
}

/**
 * A 28 px icon button for a fullscreen row; its label is its accessible name and
 * tooltip. Without `onClick` it only keeps the row's columns aligned.
 */
export function IconButton({
  icon,
  label,
  onClick,
  disabled
}: {
  icon: IconName;
  label: string;
  onClick?: (() => void) | undefined;
  disabled?: boolean;
}) {
  if (!onClick) return <span class="ib gap" aria-hidden="true" />;
  return (
    <button
      type="button"
      class="ib"
      aria-label={label}
      title={label}
      disabled={disabled}
      onClick={onClick}
    >
      <Icon name={icon} />
    </button>
  );
}

/**
 * A button that reads as a link: URLs in rows, "Refresh", "All lines", "Show all".
 * `expanded` is set on buttons that show and hide more of the card.
 */
export function LinkButton({
  onClick,
  title,
  expanded,
  children
}: {
  onClick: () => void;
  title?: string;
  expanded?: boolean;
  children: ComponentChildren;
}) {
  return (
    <button type="button" class="lk" title={title} aria-expanded={expanded} onClick={onClick}>
      {children}
    </button>
  );
}
