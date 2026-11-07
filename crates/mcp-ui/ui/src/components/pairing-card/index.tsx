// The pairing card: the one-time code `daemon pair` made, the command to run on the
// other machine, a countdown and a copy button. The tool's text carries the code too,
// so hosts that show no card still give it to the person.

import { useEffect, useState } from 'preact/hooks';
import { textOf } from '../../bridge';
import { act, tellModel } from '../../store/actions';
import { card, readOnly, structured } from '../../store/card';
import type { Pairing } from '../../types/proto';
import { Button } from '../button';
import { Actions, Card, Head, Line, Pill } from '../card';

/**
 * Copies text, falling back to a selection and `execCommand` where the host grants no
 * clipboard permission. Resolves false when both are refused.
 */
async function copy(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    const area = document.createElement('textarea');
    area.value = text;
    document.body.append(area);
    area.select();
    let ok = false;
    try {
      ok = document.execCommand('copy');
    } catch {
      ok = false;
    }
    area.remove();
    return ok;
  }
}

/** `9:58`. */
function countdown(ms: number): string {
  const s = Math.max(0, Math.ceil(ms / 1000));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
}

/** The pairing card. */
export function PairingCard() {
  const sc = structured() || {};
  const [pairing, setPairing] = useState<{ data: Pairing | null; at: number }>({
    data: sc.data ?? null,
    at: card.resultAt || Date.now()
  });
  const [, setTick] = useState(0);
  const [copied, setCopied] = useState<'yes' | 'no' | null>(null);
  useEffect(() => {
    const timer = setInterval(() => setTick((n) => n + 1), 1000);
    return () => clearInterval(timer);
  }, []);

  const data = pairing.data || {};
  const code = data.code;
  // From the time the code arrived, so the countdown needs no clock shared with the box.
  const left = (data.valid_ms ?? 0) - (Date.now() - pairing.at);
  const expired = !code || left <= 0;
  const command = `devlanes remote connect BOX --code ${code}`;
  const renew = async () => {
    const out = await act('pair', undefined);
    if (out && !out.exit_code && out.data) {
      setPairing({ data: out.data, at: Date.now() });
      setCopied(null);
      tellModel('user made a new pairing code from the card');
    }
  };
  const onCopy = async () => {
    const ok = await copy(command);
    setCopied(ok ? 'yes' : 'no');
    if (ok) setTimeout(() => setCopied((c) => (c === 'yes' ? null : c)), 2000);
  };

  if (!code) {
    return (
      <Card label="pairing code">
        <Head title="Pair a machine" pill={<Pill tone="danger">No code</Pill>} />
        <Line tone="danger">
          {textOf(card.result).split('\n')[0] || 'devlanes made no pairing code.'}
        </Line>
      </Card>
    );
  }
  return (
    <Card label="pairing code">
      <Head
        title="Pair a machine"
        pill={expired ? <Pill>Expired</Pill> : <Pill>Expires in {countdown(left)}</Pill>}
      />
      <p class={`code${expired ? ' expired' : ''}`} aria-live="polite">
        {code}
      </p>
      {!expired && <p class="well cmd">{command}</p>}
      {copied === 'no' && (
        <Line tone="warn">The host blocked the clipboard. Select the command and copy it.</Line>
      )}
      <Actions>
        {expired ? (
          !readOnly() && (
            <Button
              key="new"
              label={card.busy.pair ? 'Making a code…' : 'New code'}
              kind="primary"
              disabled={!!card.busy.pair}
              onClick={renew}
            />
          )
        ) : (
          <Button
            key="copy"
            label={copied === 'yes' ? 'Copied' : 'Copy command'}
            kind="primary"
            onClick={onCopy}
          />
        )}
      </Actions>
    </Card>
  );
}
