import { textOf } from '../../bridge';
import { card, structured } from '../../store/card';
import { commandLine } from '../../utils/format';
import { FullscreenButton } from '../fullscreen-button';
import { Window } from '../window';

/**
 * A result without a card of its own, or a command that failed before it had
 * anything to show: the command and its text, as the agent got it.
 */
export function TextCard() {
  const sc = structured() || {};
  const text = textOf(card.result);
  const failed = card.result?.isError || sc.exit_code;
  return (
    <Window
      title={<b>devlanes {sc.command || ''}</b>}
      label="devlanes output"
      tools={<FullscreenButton />}
    >
      <p class="cmd">$ {commandLine(sc.args || card.input?.args || [])}</p>
      <pre class={`out${failed ? ' tone-failure' : ''}`}>{text || 'no output'}</pre>
      {sc.exit_code ? <p class="tone-failure">exit {sc.exit_code}</p> : null}
    </Window>
  );
}
