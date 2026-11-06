// @ts-check
// The fake MCP Apps host of a preview page. It answers the card's `ui/initialize` with a
// host context carrying the chosen host's style variables (hosts.js), sends the
// fixture's tool input and result, answers app-only tool calls from the fixture's rules
// and logs every message both ways. Switching the host, theme or width sends
// `ui/notifications/host-context-changed` the way that host does. `window.PREVIEW` is
// the fixture and the card's HTML, written into the page by tests/it/preview.rs.
(() => {
  /** @type {any} */
  const fixture = /** @type {any} */ (window).PREVIEW;
  /** @type {{hosts: Record<string, any>, variables: (name: string, theme: 'light' | 'dark') => Record<string, string>}} */
  const { hosts, variables } = /** @type {any} */ (window).HOSTS;
  const params = new URLSearchParams(location.search);
  let host = hosts[params.get('host') || ''] ? String(params.get('host')) : 'devlanes';
  /** @type {'light' | 'dark'} */
  let theme = params.get('theme') === 'dark' ? 'dark' : 'light';
  let width = Number(params.get('width') || fixture.width || 0);
  const frame = /** @type {HTMLIFrameElement} */ (document.getElementById('card'));
  const box = /** @type {HTMLElement} */ (document.getElementById('box'));
  const log = /** @type {HTMLElement} */ (document.getElementById('log'));
  const shift = Date.now() - fixture.now;
  let displayMode = fixture.displayMode || 'inline';
  let initialized = false;
  /** @type {Map<number, number>} */
  const served = new Map();
  /** @type {HTMLStyleElement} */
  const hostFonts = document.createElement('style');
  document.head.append(hostFonts);

  /** The display modes the current host lists: VS Code offers inline only. */
  function modes() {
    return hosts[host].fullscreen ? fixture.displayModes : ['inline'];
  }

  /** ChatGPT keeps its composer over the bottom of a fullscreen app. */
  function insets() {
    return { top: 0, right: 0, bottom: host === 'chatgpt' && displayMode === 'fullscreen' ? 92 : 0, left: 0 };
  }

  /** The `styles` the current host sends. */
  function styles() {
    return { variables: variables(host, theme), css: { fonts: hosts[host].fonts } };
  }

  /**
   * Dresses the page as the host's chat: its variables on this document too, so the
   * frame and the messages use them, its page colour, and a frame for prefersBorder.
   */
  function dress() {
    const root = document.documentElement;
    root.dataset.theme = theme;
    root.dataset.host = host;
    root.style.colorScheme = theme;
    for (const name of [...root.style]) if (name.startsWith('--')) root.style.removeProperty(name);
    for (const [name, value] of Object.entries(variables(host, theme))) root.style.setProperty(name, value);
    root.style.setProperty('--page', hosts[host].page[theme === 'dark' ? 1 : 0]);
    hostFonts.textContent = hosts[host].fonts;
    box.classList.toggle('bordered', hosts[host].border);
    box.classList.toggle('fullscreen', displayMode === 'fullscreen');
    box.style.width = width ? `${width}px` : '';
    for (const select of document.querySelectorAll('select[data-switch]')) {
      const el = /** @type {HTMLSelectElement} */ (select);
      el.value = el.dataset.switch === 'host' ? host : el.dataset.switch === 'theme' ? theme : String(width || '');
    }
  }

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
   * Tells the card part of its context changed.
   * @param {Record<string, unknown>} change
   */
  function changed(change) {
    if (initialized) send({ jsonrpc: '2.0', method: 'ui/notifications/host-context-changed', params: change });
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
    const index = fixture.tools.findIndex((/** @type {any} */ rule) => rule.name === call.name && fits(rule.match, args));
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

  /**
   * Clicks the card's button whose label starts with `text`, for the fixture's steps.
   * The frame shares this page's origin in the preview only, so this can reach in.
   * @param {string} text
   */
  function click(text) {
    const doc = frame.contentDocument;
    const button = [...(doc?.querySelectorAll('button') || [])].find((b) => (b.textContent || '').trim().startsWith(text));
    if (button) button.click();
    else console.warn(`preview step: no button "${text}"`);
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
          hostInfo: { name: `devlanes-preview-${host}`, version: '1' },
          hostCapabilities: { openLinks: {}, serverTools: {}, logging: {} },
          hostContext: {
            theme,
            styles: styles(),
            displayMode,
            availableDisplayModes: modes(),
            containerDimensions: width ? { width, maxHeight: 4000 } : { maxHeight: 4000 },
            safeAreaInsets: insets(),
            platform: 'desktop',
            locale: 'en-US',
          },
        });
        break;
      case 'ui/notifications/initialized':
        initialized = true;
        setTimeout(() => send({ jsonrpc: '2.0', method: 'ui/notifications/tool-input', params: { arguments: fixture.input } }), 100);
        if (fixture.result) setTimeout(sendResult, 100 + fixture.resultDelayMs);
        for (const step of fixture.steps || []) setTimeout(() => click(step.click), step.afterMs);
        break;
      case 'tools/call':
        setTimeout(() => respond(message.id, answer(message.params)), 150);
        break;
      case 'ui/request-display-mode': {
        const want = message.params.mode;
        if (modes().includes(want)) displayMode = want;
        dress();
        respond(message.id, { mode: displayMode });
        changed({ displayMode, safeAreaInsets: insets() });
        break;
      }
      case 'ui/notifications/size-changed':
        if (displayMode === 'inline') frame.style.height = `${message.params.height}px`;
        break;
      default:
        if (message.id !== undefined && message.method) respond(message.id, {});
    }
  });

  /** @type {Record<string, (value: string) => void>} */
  const switches = {
    // A real host never changes; the preview sends everything a new host would.
    host: (value) => {
      host = value;
      if (!modes().includes(displayMode)) displayMode = 'inline';
      dress();
      changed({ theme, styles: styles(), availableDisplayModes: modes(), displayMode, safeAreaInsets: insets() });
    },
    theme: (value) => {
      theme = value === 'dark' ? 'dark' : 'light';
      dress();
      // light-dark() hosts change nothing but the theme; the others resend their values.
      changed(hosts[host].themeOnly ? { theme } : { theme, styles: styles() });
    },
    width: (value) => {
      width = Number(value) || 0;
      dress();
      changed({ containerDimensions: width ? { width, maxHeight: 4000 } : { maxHeight: 4000 } });
    },
  };
  for (const select of document.querySelectorAll('select[data-switch]')) {
    const el = /** @type {HTMLSelectElement} */ (select);
    el.addEventListener('change', () => switches[el.dataset.switch || '']?.(el.value));
  }

  /** @type {Record<string, () => void>} */
  const buttons = {
    result: sendResult,
    cancel: () => send({ jsonrpc: '2.0', method: 'ui/notifications/tool-cancelled', params: { reason: 'the user stopped the agent' } }),
    teardown: () => send({ jsonrpc: '2.0', id: 9001, method: 'ui/resource-teardown', params: { reason: 'preview' } }),
    clear: () => log.replaceChildren(),
  };
  for (const [name, action] of Object.entries(buttons)) {
    document.querySelector(`[data-action="${name}"]`)?.addEventListener('click', action);
  }
  dress();
  // Hosts sandbox the card without same-origin; only a preview that clicks for itself
  // needs to reach into the frame.
  frame.sandbox.value = (fixture.steps || []).length ? 'allow-scripts allow-same-origin' : 'allow-scripts';
  frame.srcdoc = fixture.card;
})();
