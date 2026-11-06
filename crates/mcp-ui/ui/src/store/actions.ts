// What the buttons do: actions through `app_action`, lines for the model, messages to
// the agent, links and display modes.

import { textOf } from '../bridge';
import type { Action, ActionResult, ToolResult } from '../types/results';
import { bridge, card, cwd, type NoteState, update } from './card';
import { pollNow } from './poll';

/** How long the line about a failed action stays. */
const NOTE_MS = 8000;

/** The words of an action running, for the button's own label: `Stopping`. */
const DOING: Record<Action, string> = {
  start: 'Starting',
  stop: 'Stopping',
  restart: 'Restarting',
  reload: 'Reloading',
  clean: 'Cleaning',
  clean_plan: 'Planning',
  pair: 'Making a code'
};

/** A button's label while its action runs for `target`, else `label`. */
export function busyLabel(target: string, label: string, name = target): string {
  const action = card.busy[target] as Action | undefined;
  if (!action) return label;
  return action === 'pair' ? `${DOING.pair}…` : `${DOING[action]} ${name}…`;
}

/**
 * Adds a line to what the model reads on its next turn. The host keeps only the
 * latest update, so each one carries the last few lines.
 */
export function tellModel(line: string): void {
  if (card.superseded) return;
  const actions = [...card.actions, line].slice(-10);
  update({ actions });
  bridge.updateModelContext(actions.join('\n')).catch(() => {});
}

/**
 * Runs `app_action` for a button, tells the model what the person did (`done`,
 * in the past tense: `stopped acme@1:api`) and polls. Resolves with the
 * action's result, or null when the call failed.
 */
export async function act(
  action: Action,
  target: string | undefined,
  done?: string
): Promise<ActionResult | null> {
  const key = target || action;
  const started =
    target && ['start', 'restart', 'reload'].includes(action)
      ? [...new Set([...card.started, target.split(':')[0] || target])]
      : card.started;
  update({ busy: { ...card.busy, [key]: action }, note: null, started });
  try {
    // Not callTool: a command that exits nonzero comes back with isError, and
    // its structured content still says what happened.
    const result = await bridge.request<ToolResult<ActionResult>>('tools/call', {
      name: 'app_action',
      arguments: { cwd: cwd(), action, target }
    });
    const out = result.structuredContent || { exit_code: 1, text: textOf(result) };
    const first = String(out.text || '')
      .trim()
      .split('\n')[0]
      ?.replace(/^error: /, '');
    if (done) {
      const exit = out.exit_code ? `; it exited ${out.exit_code}: ${first}` : '';
      tellModel(`user ${done} from the card${exit}`);
    }
    if (out.exit_code)
      flash({ text: first || `${action} exited ${out.exit_code}`, tone: 'danger' });
    return out;
  } catch (error) {
    flash({ text: `${action} ${target || ''}: ${(error as Error).message}`, tone: 'danger' });
    return null;
  } finally {
    const busy = { ...card.busy };
    delete busy[key];
    update({ busy });
    if (action !== 'clean_plan' && action !== 'pair') pollNow();
  }
}

/** Shows a line above the buttons for a few seconds. */
export function flash(note: NoteState): void {
  update({ note });
  setTimeout(() => {
    if (card.note === note) update({ note: null });
  }, NOTE_MS);
}

/**
 * Sends a message to the agent as the person. The host shows it as their turn,
 * so the card adds nothing when it went through.
 */
export function ask(text: string): void {
  bridge
    .sendMessage(text)
    .catch((error: Error) => flash({ text: `Not sent: ${error.message}`, tone: 'danger' }));
}

/** Opens a URL through the host. */
export function openUrl(url: string): void {
  bridge
    .openLink(url)
    .catch((error: Error) => flash({ text: `Not opened: ${error.message}`, tone: 'danger' }));
}

/** Asks the host for fullscreen or back to inline, and follows what it chose. */
export async function display(mode: 'inline' | 'fullscreen'): Promise<void> {
  try {
    const chosen = await bridge.requestDisplayMode(mode);
    update({ context: { ...card.context, displayMode: chosen } });
    document.documentElement.classList.toggle('fullscreen', chosen === 'fullscreen');
    pollNow();
  } catch (error) {
    flash({ text: `No fullscreen: ${(error as Error).message}`, tone: 'danger' });
  }
}
