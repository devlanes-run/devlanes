import { Icon, type IconName } from '../icon';

/** The props of {@link Button}. */
export interface ButtonProps {
  icon?: IconName;
  label: string;
  onClick?: () => void;
  primary?: boolean;
  disabled?: boolean;
  iconOnly?: boolean;
  cls?: string;
}

/**
 * A compact site button. With `iconOnly` the label becomes its accessible name
 * and tooltip.
 */
export function Button({ icon, label, onClick, primary, disabled, iconOnly, cls }: ButtonProps) {
  const classes = `btn${primary ? ' primary' : ''}${iconOnly ? ' icon-only' : ''}${cls ? ` ${cls}` : ''}`;
  return (
    <button
      type="button"
      class={classes}
      onClick={onClick}
      disabled={disabled}
      aria-label={iconOnly ? label : undefined}
      title={iconOnly ? label : undefined}
    >
      {icon && <Icon name={icon} />}
      {!iconOnly && <span>{label}</span>}
    </button>
  );
}
