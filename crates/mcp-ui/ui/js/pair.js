// @ts-check
// The pairing card: the one-time code `daemon pair` made, its countdown and a
// copy button. The code never goes into the model context.
import { html, useEffect, useState } from '../vendor/standalone.module.js';
import { Button, Note, Window } from './ui.js';
import { FullscreenButton, act, card, readOnly, structured, tellModel } from './store.js';

/**
 * Copies text, falling back to a selection and `execCommand` where the host
 * grants no clipboard permission.
 * @param {string} text
 * @returns {Promise<boolean>}
 */
async function copy(text) {
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

/**
 * `mm:ss`.
 * @param {number} ms
 */
function clock(ms) {
  const s = Math.max(0, Math.ceil(ms / 1000));
  return `${String(Math.floor(s / 60)).padStart(2, '0')}:${String(s % 60).padStart(2, '0')}`;
}

/** The pairing card. */
export function PairView() {
  const sc = structured() || {};
  const [pairing, setPairing] = useState({ data: sc.data, at: card.resultAt || Date.now() });
  const [, setTick] = useState(0);
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    const timer = setInterval(() => setTick((n) => n + 1), 1000);
    return () => clearInterval(timer);
  }, []);

  const data = pairing.data || {};
  // From the time the code arrived, so the countdown needs no clock shared with the box.
  const left = (data.valid_ms ?? 0) - (Date.now() - pairing.at);
  const expired = !data.code || left <= 0;
  const text = (card.result && card.result.content || []).map((block) => block.text || '').join('\n').trim();
  const renew = async () => {
    const out = await act('pair', undefined);
    if (out && !out.exit_code && out.data) {
      setPairing({ data: out.data, at: Date.now() });
      tellModel('user made a new pairing code from the card');
    }
  };
  const onCopy = async () => {
    setCopied(await copy(data.code));
    setTimeout(() => setCopied(false), 2000);
  };

  const foot = html`<span class="poll">${data.code ? (expired ? 'expired' : `valid ${clock(left)}`) : ''}</span>
    <span class="actions">
      ${data.code && html`<${Button} icon="copy" label=${copied ? 'copied' : 'copy'} disabled=${expired} onClick=${onCopy} />`}
      ${!readOnly() && html`<${Button} label="new code" primary=${expired} disabled=${!!card.busy.pair} onClick=${renew} />`}
    </span>`;

  return html`<${Window} title=${html`<b>pair a machine</b>`} label="pairing code" tools=${html`<${FullscreenButton} />`} foot=${foot}>
    ${data.code
      ? html`<p class="label dim">one-time code</p>
          <p class=${`code${expired ? ' expired' : ''}`} aria-live="polite">${data.code}</p>
          <p class="dim">on the other machine, with this box's name</p>
          <p><code>devlanes remote connect BOX --code ${data.code}</code></p>`
      : html`<p class="tone-failure">${text || 'no pairing code'}</p>`}
    <${Note} note=${card.note} />
  <//>`;
}
