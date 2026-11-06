// @ts-check
// The root: connects to the host, picks the view and keeps the host told about
// the card's size.
import { html, render } from '../vendor/standalone.module.js';
import { BoardView } from './board.js';
import { CleanView } from './clean.js';
import { FailureView } from './failure.js';
import { LaneView, PendingView } from './lane.js';
import { PairView } from './pair.js';
import { FullscreenButton, bridge, card, currentView, onChange, pollNow, stopPolling, structured, update } from './store.js';
import { Window } from './ui.js';
import { commandLine } from './util.js';

/** The view for the current state. */
function App() {
  if (card.failed) {
    return html`<${Window} title=${html`<b>devlanes</b>`} label="devlanes">
      <p class="tone-attention">! ${card.failed}</p>
      <p class="dim">this card runs inside an MCP Apps host, which answers its ui/initialize</p>
    <//>`;
  }
  if (!card.connected) return html`<p class="boot">connecting to the host…</p>`;
  if (card.drill && card.drill.unit) return html`<${FailureView} />`;
  switch (currentView()) {
    case 'lane':
      return html`<${LaneView} />`;
    case 'board':
      return card.drill && card.drill.lane ? html`<${LaneView} />` : html`<${BoardView} />`;
    case 'failure':
      return html`<${FailureView} />`;
    case 'clean':
      return html`<${CleanView} />`;
    case 'pair':
      return html`<${PairView} />`;
    case 'pending':
      return html`<${PendingView} tools=${html`<${FullscreenButton} />`} />`;
    default:
      return html`<${TextView} />`;
  }
}

/** A result without a card of its own: the command and its text, as the agent got it. */
function TextView() {
  const sc = structured() || {};
  const text = ((card.result && card.result.content) || []).map((block) => block.text || '').join('\n').trim();
  const failed = (card.result && card.result.isError) || sc.exit_code;
  return html`<${Window} title=${html`<b>devlanes ${sc.command || ''}</b>`} label="devlanes output" tools=${html`<${FullscreenButton} />`}>
    <p class="cmd">$ ${commandLine(sc.args || (card.input && card.input.args) || [])}</p>
    <pre class=${`out${failed ? ' tone-failure' : ''}`}>${text || 'no output'}</pre>
    ${sc.exit_code ? html`<p class="tone-failure">exit ${sc.exit_code}</p>` : null}
  <//>`;
}

/**
 * Applies the host context: theme, fullscreen and the container's size rules.
 * @param {import('./bridge.js').HostContext} context
 */
function applyContext(context) {
  const root = document.documentElement;
  if (context.theme === 'light' || context.theme === 'dark') root.dataset.theme = context.theme;
  root.classList.toggle('fullscreen', context.displayMode === 'fullscreen');
  const dims = context.containerDimensions || {};
  root.classList.toggle('fixed-height', typeof dims.height === 'number');
  root.style.maxHeight = typeof dims.maxHeight === 'number' ? `${dims.maxHeight}px` : '';
  root.style.maxWidth = typeof dims.maxWidth === 'number' ? `${dims.maxWidth}px` : '';
  update({ context });
}

/** Wires the host's notifications, then runs the handshake. */
async function boot() {
  const root = /** @type {HTMLElement} */ (document.getElementById('app'));
  onChange(() => render(html`<${App} />`, root));
  bridge.on('ui/notifications/tool-input-partial', (params) => {
    if (!card.inputDone) update({ input: params.arguments || {} });
  });
  bridge.on('ui/notifications/tool-input', (params) => {
    update({ input: params.arguments || {}, inputDone: true });
    pollNow();
  });
  bridge.on('ui/notifications/tool-result', (params) => {
    update({ result: params, resultAt: Date.now() });
    pollNow();
  });
  bridge.on('ui/notifications/tool-cancelled', (params) => update({ cancelled: params.reason || 'by the host' }));
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
    update({ failed: /** @type {Error} */ (error).message });
  }
}

boot();
