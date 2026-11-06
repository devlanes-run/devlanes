import type { ComponentChildren } from 'preact';
import { Mark } from '../icon';

/** The props of {@link Window}. */
export interface WindowProps {
  title: ComponentChildren;
  label: string;
  tools?: ComponentChildren;
  foot?: ComponentChildren;
  children?: ComponentChildren;
}

/**
 * The website's terminal window: dots, a centred title, tools on the right,
 * the body and an optional footer.
 */
export function Window({ title, label, tools, foot, children }: WindowProps) {
  return (
    <section class="term" aria-label={label}>
      <header class="term-bar">
        <span class="dots" aria-hidden="true">
          <i />
          <i />
          <i />
        </span>
        <span class="term-title">
          <Mark />
          <span>{title}</span>
        </span>
        <span class="term-tools">{tools}</span>
      </header>
      <div class="term-body">{children}</div>
      {foot && <footer class="term-foot">{foot}</footer>}
    </section>
  );
}

/** A notched pane, as the TUI draws its boxes: a border with the title in it. */
export function Pane({
  title,
  children,
  cls
}: {
  title: ComponentChildren;
  children?: ComponentChildren;
  cls?: string;
}) {
  return (
    <section class={`pane ${cls || ''}`}>
      <h2 class="pane-title">{title}</h2>
      {children}
    </section>
  );
}
