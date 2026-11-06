// What the buttons do: actions through `app_action`, lines for the model, the
// note under the card, messages to the agent and links.

import { textOf } from '../bridge';
import type { Action, ActionResult, ToolResult } from '../types/results';
import { bridge, card, cwd, type NoteState, update } from './card';
import { pollNow } from './poll';

/** How long a note under the card stays. */
const NOTE_MS = 6000;

/**
 * Adds a line to what the model reads on its next turn. The host keeps only the
 * latest update, so each one carries the last few lines.
 */
export function tellModel(line: string): void {
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
  update({ busy: { ...card.busy, [key]: action }, note: null });
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
      .split('\n')[0];
    if (done) {
      const exit = out.exit_code ? `; it exited ${out.exit_code}: ${first}` : '';
      tellModel(`user ${done} from the card${exit}`);
    }
    if (out.exit_code)
      flash({ text: first || `${action} exited ${out.exit_code}`, tone: 'failure' });
    return out;
  } catch (error) {
    flash({ text: `${action} ${target || ''}: ${(error as Error).message}`, tone: 'failure' });
    return null;
  } finally {
    const busy = { ...card.busy };
    delete busy[key];
    update({ busy });
    if (action !== 'clean_plan' && action !== 'pair') pollNow();
  }
}

/** Shows a note under the card for a few seconds. */
export function flash(note: NoteState): void {
  update({ note });
  setTimeout(() => {
    if (card.note === note) update({ note: null });
  }, NOTE_MS);
}

/** Sends a message to the agent as the person. */
export function ask(text: string): void {
  bridge
    .sendMessage(text)
    .then(() => flash({ text: 'sent to the agent' }))
    .catch((error: Error) => flash({ text: `not sent: ${error.message}`, tone: 'failure' }));
}

/** Opens a URL through the host. */
export function openUrl(url: string): void {
  bridge
    .openLink(url)
    .catch((error: Error) => flash({ text: `not opened: ${error.message}`, tone: 'failure' }));
}
