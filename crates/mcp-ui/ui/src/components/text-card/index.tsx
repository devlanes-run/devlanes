import { textOf } from '../../bridge';
import { card, structured } from '../../store/card';
import { Card, Head, Line } from '../card';

/**
 * A result its view has nothing to show for, such as a command that failed before it
 * had a lane: the command and the first line of its text. The host shows the rest.
 */
export function TextCard() {
  const sc = structured() || {};
  const first = textOf(card.result).split('\n')[0] || 'No output.';
  const failed = card.result?.isError || !!sc.exit_code;
  return (
    <Card label="devlanes output">
      <Head title={`devlanes ${sc.command || ''}`.trim()} />
      <Line tone={failed ? 'danger' : undefined}>{first}</Line>
    </Card>
  );
}
