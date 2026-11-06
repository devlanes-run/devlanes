// @ts-check
// The MCP Apps bridge: JSON-RPC 2.0 over postMessage with the host window, as the
// stable spec (2026-01-26) describes it. Written by hand because the official SDK
// needs a bundler and zod.

/** The MCP Apps protocol version the card speaks. */
const PROTOCOL_VERSION = '2026-01-26';

/** How long the host gets to answer `ui/initialize` before the card says so. */
const INITIALIZE_TIMEOUT_MS = 10000;
/** How long `callTool` waits; a host that drops an answer would stop polling for good. */
const CALL_TIMEOUT_MS = 30000;

/**
 * @typedef {object} HostContext
 * @property {'light' | 'dark'} [theme]
 * @property {string} [displayMode]
 * @property {string[]} [availableDisplayModes]
 * @property {{width?: number, maxWidth?: number, height?: number, maxHeight?: number}} [containerDimensions]
 */

/**
 * @typedef {object} ToolResult
 * @property {{type: string, text?: string}[]} [content]
 * @property {any} [structuredContent]
 * @property {boolean} [isError]
 */

export class Bridge {
  /**
   * Listens to messages from `target`, the host window, and ignores every other
   * window, so a page that embeds the host cannot drive the card.
   * @param {Window} target
   */
  constructor(target) {
    this.target = target;
    this.nextId = 1;
    /** @type {Map<number, {resolve: (value: any) => void, reject: (error: Error) => void}>} */
    this.pending = new Map();
    /** @type {Map<string, (params: any) => any>} */
    this.handlers = new Map();
    /** @type {HostContext} */
    this.context = {};
    /** @type {Record<string, any>} */
    this.capabilities = {};
    /** @type {{width: number, height: number} | null} */
    this.lastSize = null;
    window.addEventListener('message', (event) => this.receive(event));
  }

  /**
   * Routes one message: responses settle their request, requests and
   * notifications go to the handler registered for their method.
   * @param {MessageEvent} event
   */
  receive(event) {
    if (event.source !== this.target) return;
    const message = event.data;
    if (!message || message.jsonrpc !== '2.0') return;
    if (message.method === undefined) {
      const waiting = this.pending.get(message.id);
      if (!waiting) return;
      this.pending.delete(message.id);
      if (message.error) waiting.reject(new Error(message.error.message || 'the host refused'));
      else waiting.resolve(message.result ?? {});
      return;
    }
    const handler = this.handlers.get(message.method);
    if (message.id === undefined) {
      if (handler) handler(message.params ?? {});
      return;
    }
    if (message.method === 'ping') {
      this.respond(message.id, {});
    } else if (handler) {
      Promise.resolve()
        .then(() => handler(message.params ?? {}))
        .then(
          (result) => this.respond(message.id, result ?? {}),
          (error) => this.respond(message.id, undefined, { code: -32000, message: String(error) }),
        );
    } else {
      this.respond(message.id, undefined, { code: -32601, message: `unknown method ${message.method}` });
    }
  }

  /**
   * Answers a request from the host.
   * @param {number | string} id
   * @param {any} result
   * @param {{code: number, message: string}} [error]
   */
  respond(id, result, error) {
    this.post(error ? { jsonrpc: '2.0', id, error } : { jsonrpc: '2.0', id, result });
  }

  /**
   * Sends one message. The target origin is `*` because a sandboxed host gives
   * the card no origin to name; `receive` checks the sender instead.
   * @param {object} message
   */
  post(message) {
    this.target.postMessage(message, '*');
  }

  /**
   * Sends a request and resolves with its result.
   * @param {string} method
   * @param {object} params
   * @param {number} [timeoutMs] rejects when the host has not answered by then
   * @returns {Promise<any>}
   */
  request(method, params, timeoutMs) {
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      if (timeoutMs) {
        setTimeout(() => {
          if (this.pending.delete(id)) reject(new Error(`${method}: no answer from the host`));
        }, timeoutMs);
      }
      this.post({ jsonrpc: '2.0', id, method, params });
    });
  }

  /**
   * Sends a notification.
   * @param {string} method
   * @param {object} params
   */
  notify(method, params) {
    this.post({ jsonrpc: '2.0', method, params });
  }

  /**
   * Registers the handler of a host notification or request; a request's
   * handler returns its result.
   * @param {string} method
   * @param {(params: any) => any} handler
   */
  on(method, handler) {
    this.handlers.set(method, handler);
  }

  /**
   * Runs the `ui/initialize` handshake and keeps the host context up to date.
   * @param {(context: HostContext) => void} onContext called with the merged context on every change
   * @returns {Promise<HostContext>}
   */
  async connect(onContext) {
    this.on('ui/notifications/host-context-changed', (params) => {
      this.context = { ...this.context, ...params };
      onContext(this.context);
    });
    const result = await this.request(
      'ui/initialize',
      {
        appInfo: { name: 'devlanes', version: VERSION },
        appCapabilities: { availableDisplayModes: ['inline', 'fullscreen'] },
        protocolVersion: PROTOCOL_VERSION,
      },
      INITIALIZE_TIMEOUT_MS,
    );
    this.context = result.hostContext || {};
    this.capabilities = result.hostCapabilities || {};
    this.notify('ui/notifications/initialized', {});
    onContext(this.context);
    return this.context;
  }

  /**
   * Calls a tool of the devlanes MCP server through the host. A result with
   * `isError` rejects with its text, and so does no answer in 30 seconds.
   * @param {string} name
   * @param {object} args
   * @returns {Promise<ToolResult>}
   */
  async callTool(name, args) {
    /** @type {ToolResult} */
    const result = await this.request('tools/call', { name, arguments: args }, CALL_TIMEOUT_MS);
    if (result.isError) {
      const text = (result.content || []).map((block) => block.text || '').join('\n').trim();
      throw new Error(text || `${name} failed`);
    }
    return result;
  }

  /**
   * Replaces what the card tells the model on its next turn.
   * @param {string} text
   */
  updateModelContext(text) {
    return this.request('ui/update-model-context', { content: [{ type: 'text', text }] });
  }

  /**
   * Sends a message to the chat as the user.
   * @param {string} text
   */
  sendMessage(text) {
    return this.request('ui/message', { role: 'user', content: [{ type: 'text', text }] });
  }

  /**
   * Asks the host to open a URL in the browser.
   * @param {string} url
   */
  openLink(url) {
    return this.request('ui/open-link', { url });
  }

  /**
   * Asks for a display mode and resolves with the one the host chose.
   * @param {'inline' | 'fullscreen'} mode
   * @returns {Promise<string>}
   */
  async requestDisplayMode(mode) {
    const result = await this.request('ui/request-display-mode', { mode });
    return result.mode || this.context.displayMode || 'inline';
  }

  /**
   * Tells the host the card's size, once per change.
   * @param {number} width
   * @param {number} height
   */
  reportSize(width, height) {
    if (this.lastSize && this.lastSize.width === width && this.lastSize.height === height) return;
    this.lastSize = { width, height };
    this.notify('ui/notifications/size-changed', { width, height });
  }
}
