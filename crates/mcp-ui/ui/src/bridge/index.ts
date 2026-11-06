// The MCP Apps bridge: JSON-RPC 2.0 over postMessage with the host window, as the
// stable spec (2026-01-26) describes it. Written by hand rather than with
// @modelcontextprotocol/ext-apps, which pulls in zod and the MCP SDK and would
// put the card over its size budget (see the crate README).

import type { ToolResult } from '../types/results';

/** The MCP Apps protocol version the card speaks. */
const PROTOCOL_VERSION = '2026-01-26';

/** How long the host gets to answer `ui/initialize` before the card says so. */
const INITIALIZE_TIMEOUT_MS = 10_000;
/** How long `callTool` waits; a host that drops an answer would stop polling for good. */
const CALL_TIMEOUT_MS = 30_000;

/** The host context of `ui/initialize` and `ui/notifications/host-context-changed`. */
export interface HostContext {
  theme?: 'light' | 'dark';
  /** Standard style variables (`--color-text-primary`...) and `@font-face` rules for them. */
  styles?: {
    variables?: Record<string, string | undefined>;
    css?: { fonts?: string };
  };
  displayMode?: string;
  availableDisplayModes?: string[];
  containerDimensions?: { width?: number; maxWidth?: number; height?: number; maxHeight?: number };
  /** Pixels the host covers at each edge, such as a mobile notch or a fullscreen composer. */
  safeAreaInsets?: { top?: number; right?: number; bottom?: number; left?: number };
}

/** A JSON-RPC error object. */
interface RpcErrorObject {
  code: number;
  message: string;
}

/** Any JSON-RPC message the host sends. */
interface Message {
  jsonrpc: '2.0';
  id?: number | string;
  method?: string;
  params?: any;
  result?: any;
  error?: { message?: string };
}

/** A request waiting for the host's answer. */
interface Pending {
  resolve: (value: any) => void;
  reject: (error: Error) => void;
}

/** A handler of a host notification or request; a request's handler returns its result. */
type Handler = (params: any) => unknown;

/**
 * The devlanes release the card ships with. The Rust side fills the meta tag when
 * it serves the card, so the committed HTML does not change with every release.
 */
function version(): string {
  const meta = document.querySelector<HTMLMetaElement>('meta[name="devlanes-version"]');
  return meta?.content ?? 'dev';
}

/** The card's side of the MCP Apps protocol. */
export class Bridge {
  private readonly target: Window;
  private nextId = 1;
  private readonly pending = new Map<number, Pending>();
  private readonly handlers = new Map<string, Handler>();
  private lastSize: { width: number; height: number } | null = null;
  context: HostContext = {};
  capabilities: Record<string, unknown> = {};

  /**
   * Listens to messages from `target`, the host window, and ignores every other
   * window, so a page that embeds the host cannot drive the card.
   */
  constructor(target: Window) {
    this.target = target;
    window.addEventListener('message', (event) => this.receive(event));
  }

  /**
   * Routes one message: responses settle their request, requests and
   * notifications go to the handler registered for their method.
   */
  private receive(event: MessageEvent): void {
    if (event.source !== this.target) return;
    const message = event.data as Message | null;
    if (message?.jsonrpc !== '2.0') return;
    if (message.method === undefined) {
      const waiting = this.pending.get(message.id as number);
      if (!waiting) return;
      this.pending.delete(message.id as number);
      if (message.error) waiting.reject(new Error(message.error.message || 'the host refused'));
      else waiting.resolve(message.result ?? {});
      return;
    }
    const handler = this.handlers.get(message.method);
    const id = message.id;
    if (id === undefined) {
      handler?.(message.params ?? {});
      return;
    }
    if (message.method === 'ping') {
      this.respond(id, {});
    } else if (handler) {
      Promise.resolve()
        .then(() => handler(message.params ?? {}))
        .then(
          (result) => this.respond(id, result ?? {}),
          (error) => this.respond(id, undefined, { code: -32000, message: String(error) })
        );
    } else {
      this.respond(id, undefined, { code: -32601, message: `unknown method ${message.method}` });
    }
  }

  /** Answers a request from the host. */
  private respond(id: number | string, result: unknown, error?: RpcErrorObject): void {
    this.post(error ? { jsonrpc: '2.0', id, error } : { jsonrpc: '2.0', id, result });
  }

  /**
   * Sends one message. The target origin is `*` because a sandboxed host gives
   * the card no origin to name; `receive` checks the sender instead.
   */
  private post(message: object): void {
    this.target.postMessage(message, '*');
  }

  /**
   * Sends a request and resolves with its result; with `timeoutMs` it rejects
   * when the host has not answered by then.
   */
  request<T = any>(method: string, params: object, timeoutMs?: number): Promise<T> {
    const id = this.nextId++;
    return new Promise<T>((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      if (timeoutMs) {
        setTimeout(() => {
          if (this.pending.delete(id)) reject(new Error(`${method}: no answer from the host`));
        }, timeoutMs);
      }
      this.post({ jsonrpc: '2.0', id, method, params });
    });
  }

  /** Sends a notification. */
  notify(method: string, params: object): void {
    this.post({ jsonrpc: '2.0', method, params });
  }

  /** Registers the handler of a host notification or request. */
  on(method: string, handler: Handler): void {
    this.handlers.set(method, handler);
  }

  /**
   * Runs the `ui/initialize` handshake and keeps the host context up to date;
   * `onContext` gets the merged context on every change.
   */
  async connect(onContext: (context: HostContext) => void): Promise<HostContext> {
    this.on('ui/notifications/host-context-changed', (params: HostContext) => {
      this.context = { ...this.context, ...params };
      onContext(this.context);
    });
    const result = await this.request<{
      hostContext?: HostContext;
      hostCapabilities?: Record<string, unknown>;
    }>(
      'ui/initialize',
      {
        appInfo: { name: 'devlanes', version: version() },
        appCapabilities: { availableDisplayModes: ['inline', 'fullscreen'] },
        protocolVersion: PROTOCOL_VERSION
      },
      INITIALIZE_TIMEOUT_MS
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
   */
  async callTool<T = any>(name: string, args: object): Promise<ToolResult<T>> {
    const result = await this.request<ToolResult<T>>(
      'tools/call',
      { name, arguments: args },
      CALL_TIMEOUT_MS
    );
    if (result.isError) {
      throw new Error(textOf(result) || `${name} failed`);
    }
    return result;
  }

  /** Replaces what the card tells the model on its next turn. */
  updateModelContext(text: string): Promise<unknown> {
    return this.request('ui/update-model-context', { content: [{ type: 'text', text }] });
  }

  /** Sends a message to the chat as the user. */
  sendMessage(text: string): Promise<unknown> {
    return this.request('ui/message', { role: 'user', content: [{ type: 'text', text }] });
  }

  /** Asks the host to open a URL in the browser. */
  openLink(url: string): Promise<unknown> {
    return this.request('ui/open-link', { url });
  }

  /** Asks for a display mode and resolves with the one the host chose. */
  async requestDisplayMode(mode: 'inline' | 'fullscreen'): Promise<string> {
    const result = await this.request<{ mode?: string }>('ui/request-display-mode', { mode });
    return result.mode || this.context.displayMode || 'inline';
  }

  /** Tells the host the card's size, once per change. */
  reportSize(width: number, height: number): void {
    if (this.lastSize && this.lastSize.width === width && this.lastSize.height === height) return;
    this.lastSize = { width, height };
    this.notify('ui/notifications/size-changed', { width, height });
  }
}

/** The text blocks of a tool result, joined. */
export function textOf(result: ToolResult | null | undefined): string {
  return (result?.content || [])
    .map((block) => block.text || '')
    .join('\n')
    .trim();
}
