// The pairing card: the one-time code `daemon pair` made, its countdown and a
// copy button. The code never goes into the model context.

import { useEffect, useState } from 'preact/hooks';
import { textOf } from '../../bridge';
import { act, tellModel } from '../../store/actions';
import { card, readOnly, structured } from '../../store/card';
import type { Pairing } from '../../types/proto';
import { Button } from '../button';
import { FullscreenButton } from '../fullscreen-button';
import { Note } from '../note';
import { Window } from '../window';

/**
 * Copies text, falling back to a selection and `execCommand` where the host
 * grants no clipboard permission.
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
    const ok = document.execCommand('copy');
    area.remove();
    return ok;
  }
}

/** `mm:ss`. */
function clock(ms: number): string {
  const s = Math.max(0, Math.ceil(ms / 1000));
  return `${String(Math.floor(s / 60)).padStart(2, '0')}:${String(s % 60).padStart(2, '0')}`;
}

/** The pairing card. */
export function PairingCard() {
  const sc = structured() || {};
  const [pairing, setPairing] = useState<{ data: Pairing | null; at: number }>({
    data: sc.data ?? null,
    at: card.resultAt || Date.now()
  });
  const [, setTick] = useState(0);
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    const timer = setInterval(() => setTick((n) => n + 1), 1000);
    return () => clearInterval(timer);
  }, []);

  const data = pairing.data || {};
  const code = data.code;
  // From the time the code arrived, so the countdown needs no clock shared with the box.
  const left = (data.valid_ms ?? 0) - (Date.now() - pairing.at);
  const expired = !code || left <= 0;
  const renew = async () => {
    const out = await act('pair', undefined);
    if (out && !out.exit_code && out.data) {
      setPairing({ data: out.data, at: Date.now() });
      tellModel('user made a new pairing code from the card');
    }
  };
  const onCopy = async () => {
    if (!code) return;
    setCopied(await copy(code));
    setTimeout(() => setCopied(false), 2000);
  };

  const foot = (
    <>
      <span class="poll">{code ? (expired ? 'expired' : `valid ${clock(left)}`) : ''}</span>
      <span class="actions">
        {code && (
          <Button
            icon="copy"
            label={copied ? 'copied' : 'copy'}
            disabled={expired}
            onClick={onCopy}
          />
        )}
        {!readOnly() && (
          <Button label="new code" primary={expired} disabled={!!card.busy.pair} onClick={renew} />
        )}
      </span>
    </>
  );

  return (
    <Window
      title={<b>pair a machine</b>}
      label="pairing code"
      tools={<FullscreenButton />}
      foot={foot}
    >
      {code ? (
        <>
          <p class="label dim">one-time code</p>
          <p class={`code${expired ? ' expired' : ''}`} aria-live="polite">
            {code}
          </p>
          <p class="dim">on the other machine, with this box's name</p>
          <p>
            <code>
              devlanes remote connect BOX --code <span class="nowrap">{code}</span>
            </code>
          </p>
        </>
      ) : (
        <p class="tone-failure">{textOf(card.result) || 'no pairing code'}</p>
      )}
      <Note note={card.note} />
    </Window>
  );
}
