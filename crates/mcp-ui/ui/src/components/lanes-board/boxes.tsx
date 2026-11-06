import type { HostLink } from '../../types/proto';
import { Glyph } from '../glyph';

/**
 * The boxes this machine is linked to. Their lanes run on the box, so the card
 * shows them and offers no buttons.
 */
export function Boxes({ hosts }: { hosts: HostLink[] }) {
  return (
    <section class="boxes" aria-label="boxes">
      <h2 class="label">
        boxes <span class="dim">· read-only here, manage their lanes on the box</span>
      </h2>
      <ul>
        {hosts.map((host) => (
          <li key={host.host}>
            <Glyph state={host.state} /> <b>{host.host}</b>{' '}
            <span class="dim">
              {[String(host.state || '').replace(/_/g, ' '), host.error]
                .filter(Boolean)
                .join(' · ')}
            </span>
            {host.fix && <p class="fix dim">run {fixWords(host.fix)}</p>}
          </li>
        ))}
      </ul>
    </section>
  );
}

/**
 * A box's `fix` with only its commands as code, from the daemon's
 * `devlanes daemon pair on fork-b, then devlanes remote connect fork-b --code <code>`.
 */
function fixWords(fix: string) {
  return fix.split(', then ').map((step, i) => {
    const [, command, where] = /^(.*?)(?: on (\S+))?$/.exec(step.replace('<code>', 'CODE')) || [];
    return (
      <>
        {i ? ', then ' : ''}
        <code>{command}</code>
        {where && ` on ${where}`}
      </>
    );
  });
}
