// @ts-check
// The fake MCP Apps host of a preview page. It answers the card's `ui/initialize`
// with a host context, sends the fixture's tool input and result, answers app-only
// tool calls from the fixture's rules and logs every message both ways.
// `window.PREVIEW` is the fixture and the card's HTML, written into the page by
// tests/it/preview.rs.
(() => {
  /** @type {any} */
  const fixture = /** @type {any} */ (window).PREVIEW;
  const params = new URLSearchParams(location.search);
  let theme = params.get('theme') || fixture.theme;
  const width = Number(params.get('width') || fixture.width || 0);
  const frame = /** @type {HTMLIFrameElement} */ (document.getElementById('card'));
  const box = /** @type {HTMLElement} */ (document.getElementById('box'));
  const log = /** @type {HTMLElement} */ (document.getElementById('log'));
  const shift = Date.now() - fixture.now;
  let displayMode = 'inline';
  let initialized = false;
  /** @type {Map<number, number>} */
  const served = new Map();

  document.documentElement.dataset.theme = theme;
  if (width) box.style.width = `${width}px`;

  /**
   * Moves every absolute time in a fixture by `shift`, so ages stay as written.
   * @param {any} value
   * @param {string} [key]
   * @returns {any}
   */
  function rebase(value, key) {
    if (Array.isArray(value)) {
      if (key === 'history') return value.map((row) => (Array.isArray(row) ? [row[0] + shift, ...row.slice(1)] : row));
      return value.map((item) => rebase(item));
    }
    if (value && typeof value === 'object') {
      return Object.fromEntries(Object.entries(value).map(([k, v]) => [k, rebase(v, k)]));
    }
    if (typeof value === 'number' && key && key.endsWith('_ms') && value > 1e12) return value + shift;
    return value;
  }

  /**
   * Adds a message to the log panel.
   * @param {'in' | 'out'} direction `in` from the card, `out` to it
   * @param {any} message
   */
  function show(direction, message) {
    const item = document.createElement('li');
    item.className = direction;
    const what = message.method || (message.error ? `error ${message.id}` : `result ${message.id}`);
    const tool = message.method === 'tools/call' ? ` ${message.params.name}` : '';
    const details = document.createElement('details');
    const summary = document.createElement('summary');
    summary.textContent = `${direction === 'in' ? 'card -> host' : 'host -> card'}  ${what}${tool}`;
    const body = document.createElement('pre');
    body.textContent = JSON.stringify(message.params ?? message.result ?? message.error ?? {}, null, 2);
    details.append(summary, body);
    item.append(details);
    log.append(item);
    log.scrollTop = log.scrollHeight;
  }

  /**
   * Sends a message to the card.
   * @param {any} message
   */
  function send(message) {
    show('out', message);
    frame.contentWindow?.postMessage(message, '*');
  }

  /**
   * Answers a request of the card.
   * @param {number} id
   * @param {any} result
   */
  function respond(id, result) {
    send({ jsonrpc: '2.0', id, result });
  }

  /**
   * Whether a rule's `match` fits a tool call's arguments: equal values, `*`
   * for any value, `prefix*` for a string prefix.
   * @param {Record<string, any>} match
   * @param {Record<string, any>} args
   */
  function fits(match, args) {
    return Object.entries(match).every(([key, want]) => {
      const got = args[key];
      if (want === '*') return got !== undefined && got !== '';
      if (typeof want === 'string' && want.endsWith('*')) return typeof got === 'string' && got.startsWith(want.slice(0, -1));
      return JSON.stringify(got) === JSON.stringify(want);
    });
  }

  /**
   * The fixture's answer to a tools/call, or an error result.
   * @param {{name: string, arguments?: any}} call
   */
  function answer(call) {
    const args = call.arguments || {};
    const index = fixture.tools.findIndex((rule) => rule.name === call.name && fits(rule.match, args));
    if (index < 0) return { content: [{ type: 'text', text: `the preview has no answer for ${call.name}` }], isError: true };
    const rule = fixture.tools[index];
    const n = served.get(index) || 0;
    served.set(index, n + 1);
    const result = rebase(rule.results[Math.min(n, rule.results.length - 1)]);
    // The server runs `debug top` only when the card asks for metrics.
    if (call.name === 'app_snapshot' && !args.metrics && result.structuredContent) result.structuredContent.top = null;
    return result;
  }

  let resultSent = false;
  /** Sends the tool result, once. */
  function sendResult() {
    if (!fixture.result || resultSent) return;
    resultSent = true;
    send({ jsonrpc: '2.0', method: 'ui/notifications/tool-result', params: rebase(fixture.result) });
  }

  window.addEventListener('message', (event) => {
    if (event.source !== frame.contentWindow) return;
    const message = event.data;
    if (!message || message.jsonrpc !== '2.0') return;
    show('in', message);
    switch (message.method) {
      case 'ui/initialize':
        respond(message.id, {
          protocolVersion: '2026-01-26',
          hostInfo: { name: 'devlanes-preview', version: '1' },
          hostCapabilities: { openLinks: {}, serverTools: {}, logging: {} },
          hostContext: {
            theme,
            displayMode,
            availableDisplayModes: fixture.displayModes,
            containerDimensions: width ? { width, maxHeight: 4000 } : { maxHeight: 4000 },
            platform: 'desktop',
            locale: 'en-US',
          },
        });
        break;
      case 'ui/notifications/initialized':
        initialized = true;
        setTimeout(() => send({ jsonrpc: '2.0', method: 'ui/notifications/tool-input', params: { arguments: fixture.input } }), 100);
        if (fixture.result) setTimeout(sendResult, 100 + fixture.resultDelayMs);
        break;
      case 'tools/call':
        setTimeout(() => respond(message.id, answer(message.params)), 150);
        break;
      case 'ui/request-display-mode': {
        const want = message.params.mode;
        if (fixture.displayModes.includes(want)) displayMode = want;
        box.classList.toggle('fullscreen', displayMode === 'fullscreen');
        respond(message.id, { mode: displayMode });
        send({ jsonrpc: '2.0', method: 'ui/notifications/host-context-changed', params: { displayMode } });
        break;
      }
      case 'ui/notifications/size-changed':
        if (displayMode === 'inline') frame.style.height = `${message.params.height}px`;
        break;
      default:
        if (message.id !== undefined && message.method) respond(message.id, {});
    }
  });

  /** @type {Record<string, () => void>} */
  const buttons = {
    theme: () => {
      theme = theme === 'dark' ? 'light' : 'dark';
      document.documentElement.dataset.theme = theme;
      if (initialized) send({ jsonrpc: '2.0', method: 'ui/notifications/host-context-changed', params: { theme } });
    },
    result: sendResult,
    cancel: () => send({ jsonrpc: '2.0', method: 'ui/notifications/tool-cancelled', params: { reason: 'the user stopped the agent' } }),
    teardown: () => send({ jsonrpc: '2.0', id: 9001, method: 'ui/resource-teardown', params: { reason: 'preview' } }),
    clear: () => log.replaceChildren(),
  };
  for (const [name, action] of Object.entries(buttons)) {
    document.querySelector(`[data-action="${name}"]`)?.addEventListener('click', action);
  }
  frame.srcdoc = fixture.card;
})();
