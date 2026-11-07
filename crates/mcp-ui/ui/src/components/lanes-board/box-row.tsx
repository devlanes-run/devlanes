import { openUrl } from '../../store/actions';
import { daemonStopped } from '../../store/card';
import type { HostLink } from '../../types/proto';
import type { BoxLane } from '../../types/remote';
import { shortUrl } from '../../utils/format';
import { laneUrl, laneWords } from '../../utils/lanes';
import { wordClass } from '../../utils/state';
import { IconButton, LinkButton } from '../button';
import { Shape } from '../card';

/** Why a box's lanes are hidden, after the TUI's box pane. */
const LINK_WORDS: Record<string, string> = {
  needs_auth: 'needs you to sign in again',
  needs_pairing: 'needs a new pairing code',
  connecting: 'is reconnecting',
  reconnecting: 'is reconnecting'
};

/** Whether a box lane can be used from this machine: its box is linked and it is mirrored. */
export function usable(lane: BoxLane): boolean {
  return lane.link === 'connected' && !lane.skipped;
}

/**
 * One line for the box lanes that cannot be used here: a box whose link is down hides
 * its lanes, and a lane this machine does not mirror is counted, except one this
 * machine runs itself, which has its own row already.
 */
export function boxWarning(boxed: BoxLane[], hosts: HostLink[]): string {
  const parts: string[] = [];
  const names = [...new Set([...hosts.map((h) => h.host), ...boxed.map((l) => l.host)])];
  for (const host of names) {
    const link =
      hosts.find((h) => h.host === host)?.state || boxed.find((l) => l.host === host)?.link;
    const lanes = boxed.filter((l) => l.host === host);
    if (link && link !== 'connected') {
      const why = LINK_WORDS[link] || `is ${link.replace(/_/g, ' ')}`;
      const count = lanes.length;
      parts.push(
        count
          ? `${host} ${why}, so its ${count === 1 ? 'lane is' : `${count} lanes are`} hidden.`
          : `${host} ${why}.`
      );
      continue;
    }
    const skipped = lanes.filter(
      (l) => l.skipped && !/runs on this machine/.test(l.skipped.reason || '')
    );
    if (skipped.length) {
      parts.push(
        `${skipped.map((l) => l.box).join(', ')} on ${host} ${skipped.length === 1 ? 'is' : 'are'} not mirrored here.`
      );
    }
  }
  return parts.join(' ');
}

/**
 * A lane of a linked box: read-only, so its row has a link that works on this machine
 * and, in fullscreen, an open button only. `remote ls` has no branch for box lanes, so
 * the row says which box runs it.
 */
export function BoxRow({ lane, manage }: { lane: BoxLane; manage: boolean }) {
  const url = daemonStopped() ? undefined : laneUrl(lane);
  return (
    <li class="row">
      <Shape state={lane.state} />
      <span class="name num">@{lane.index}</span>
      <span class="grow sec">on {lane.host}</span>
      {!manage && url && (
        <LinkButton title={url} onClick={() => openUrl(url)}>
          {shortUrl(url)}
        </LinkButton>
      )}
      <span class={`end ${wordClass(lane.state)}`}>{laneWords(lane, Date.now())}</span>
      {manage && (
        <span class="acts">
          <IconButton
            icon="open"
            label={`Open ${lane.box} on ${lane.host}`}
            onClick={url ? () => openUrl(url) : undefined}
          />
          <IconButton icon="stop" label="" />
          <IconButton icon="ask" label="" />
        </span>
      )}
    </li>
  );
}
