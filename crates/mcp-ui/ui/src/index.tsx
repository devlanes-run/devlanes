// The entry: connects to the host, renders the card on every change and keeps
// the host told about the card's size.

import { render } from 'preact';
import type { HostContext } from './bridge';
import { applyHostStyles } from './bridge/styles';
import { App } from './components/app';
import { bridge, card, onChange, update } from './store/card';
import { pollNow, stopPolling, watchActivity } from './store/poll';
import './styles/app.scss';

/** Applies the host context: styles, fullscreen and the container's size rules. */
function applyContext(context: HostContext): void {
  const root = document.documentElement;
  applyHostStyles(context);
  root.classList.toggle('fullscreen', context.displayMode === 'fullscreen');
  const dims = context.containerDimensions || {};
  root.classList.toggle('fixed-height', typeof dims.height === 'number');
  root.style.maxHeight = typeof dims.maxHeight === 'number' ? `${dims.maxHeight}px` : '';
  root.style.maxWidth = typeof dims.maxWidth === 'number' ? `${dims.maxWidth}px` : '';
  update({ context });
}

/** Wires the host's notifications, then runs the handshake. */
async function boot(): Promise<void> {
  const root = document.getElementById('app') as HTMLElement;
  onChange(() => render(<App />, root));
  watchActivity();
  bridge.on('ui/notifications/tool-input-partial', (params) => {
    if (!card.inputDone) update({ input: params.arguments || {} });
  });
  bridge.on('ui/notifications/tool-input', (params) => {
    update({ input: params.arguments || {}, inputDone: true });
    pollNow();
  });
  bridge.on('ui/notifications/tool-result', (params) => {
    update({ result: params, resultAt: Date.now(), updatedAt: Date.now() });
    pollNow();
  });
  bridge.on('ui/notifications/tool-cancelled', (params) =>
    update({ cancelled: params.reason || 'by the host' })
  );
  bridge.on('ui/resource-teardown', () => {
    stopPolling();
    return {};
  });
  // The host sizes the iframe to the card; the width stays the host's.
  new ResizeObserver(() => {
    bridge.reportSize(window.innerWidth, Math.ceil(document.body.getBoundingClientRect().height));
  }).observe(document.body);
  try {
    await bridge.connect(applyContext);
    update({ connected: true });
  } catch (error) {
    update({ failed: (error as Error).message });
  }
}

boot();
