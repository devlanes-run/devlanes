// Applies the host's theme, style variables, fonts and safe area to the document, as the
// MCP Apps spec describes `hostContext.styles`. The card's stylesheet reads only the
// standard variable names, so whatever the host sends here restyles it.

import type { HostContext } from './index';

/** The host variables set on `<html>` last time, so a change that drops one removes it. */
let applied = new Set<string>();
/** The host's `@font-face` rules, in one constructed sheet replaced when they change. */
let fontSheet: CSSStyleSheet | null = null;
let fontText = '';

/**
 * Sets `data-theme` and `color-scheme` on `<html>` (Claude, Goose and others send
 * `light-dark()` values, which resolve against it), writes every host variable onto
 * `<html>` and removes the ones the host no longer sends. `data-host-styles` marks that
 * the host sent some, which drops the devlanes accent.
 */
export function applyHostStyles(context: HostContext): void {
  const root = document.documentElement;
  if (context.theme === 'light' || context.theme === 'dark') {
    root.dataset.theme = context.theme;
    root.style.colorScheme = context.theme;
  }
  const next = new Set<string>();
  for (const [name, value] of Object.entries(context.styles?.variables || {})) {
    if (!name.startsWith('--') || typeof value !== 'string' || !value) continue;
    root.style.setProperty(name, value);
    next.add(name);
  }
  for (const name of applied) {
    if (!next.has(name)) root.style.removeProperty(name);
  }
  applied = next;
  root.toggleAttribute('data-host-styles', next.size > 0);
  applyFonts(context.styles?.css?.fonts || '');
  const insets = context.safeAreaInsets || {};
  for (const side of ['top', 'right', 'bottom', 'left'] as const) {
    const px = insets[side];
    if (typeof px === 'number' && px > 0) root.style.setProperty(`--dl-safe-${side}`, `${px}px`);
    else root.style.removeProperty(`--dl-safe-${side}`);
  }
}

/**
 * Applies the host's font rules through a constructed stylesheet: the card's policy
 * allows only its own `<style>` by hash, and a constructed sheet is not an inline
 * style, so it passes. `@import` rules are dropped by `replaceSync`; the hosts that send
 * fonts send `@font-face` only. The fonts themselves load under `font-src`.
 */
function applyFonts(css: string): void {
  if (css === fontText) return;
  fontText = css;
  try {
    if (!fontSheet) {
      fontSheet = new CSSStyleSheet();
      document.adoptedStyleSheets = [...document.adoptedStyleSheets, fontSheet];
    }
    fontSheet.replaceSync(css);
  } catch {
    // An engine without constructed sheets keeps the fallback fonts of --font-sans.
  }
}
