// The fullscreen log viewer of the failure card: every line with its cursor, a filter,
// earlier pages, and a picked line the agent gets asked about. Inline cards show an
// excerpt instead, since they must not scroll.

import type { ComponentChildren } from 'preact';
import { useEffect, useState } from 'preact/hooks';
import { ask } from '../../store/actions';
import { bridge, canHandOff, card, cwd } from '../../store/card';
import type { LogLine, LogsPage } from '../../types/proto';
import type { LogsResult } from '../../types/results';
import { Button } from '../button';
import { Actions, Line } from '../card';
import { Excerpt } from '../failure-card/excerpt';
import { Icon } from '../icon';

/** Lines the viewer keeps; older ones drop off the top while it follows. */
const MAX_LINES = 2000;
/** Lines each fetch asks for. */
const PAGE = 200;
/** How long typing in the filter settles before the viewer fetches. */
const FILTER_DEBOUNCE_MS = 300;

/** Fetches a page of one unit's logs through `app_logs`. */
async function fetchLogs(ref: string, args: Record<string, unknown>): Promise<LogsPage> {
  const result = await bridge.callTool<LogsResult>('app_logs', {
    cwd: cwd(),
    units: [ref],
    ...args
  });
  return result.structuredContent?.logs || {};
}

/** The cursor of a line, `api#118`, or null for a line that came without one. */
function cursorOf(line: LogLine | null | undefined): string | null {
  return line && line.seq != null ? `${line.unit}#${line.seq}` : null;
}

/** The props of {@link LogViewer}: the card's lines, which the viewer extends. */
interface LogViewerProps {
  unitRef: string;
  lines: LogLine[];
  setLines: (update: (old: LogLine[]) => LogLine[]) => void;
  earlier: string | null;
  setEarlier: (cursor: string | null) => void;
  restart: ComponentChildren;
}

/** The log viewer. */
export function LogViewer({
  unitRef,
  lines,
  setLines,
  earlier,
  setEarlier,
  restart
}: LogViewerProps) {
  const [filter, setFilter] = useState('');
  const [found, setFound] = useState<{ lines: LogLine[]; earlier: string | null } | null>(null);
  const [picked, setPicked] = useState<number | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState('');
  const shown = found ? found.lines : lines;
  const before = found ? found.earlier : earlier;

  // A filter reads its own matches; clearing it goes back to the card's lines.
  useEffect(() => {
    setPicked(null);
    if (!filter) {
      // A filtered fetch still in flight was cancelled by the cleanup, so its finally
      // never runs: reset what it would have reset.
      setFound(null);
      setLoading(false);
      setError('');
      return undefined;
    }
    let gone = false;
    const timer = setTimeout(async () => {
      setLoading(true);
      try {
        const page = await fetchLogs(unitRef, { tail: PAGE, grep: filter });
        if (!gone) setFound({ lines: page.items || [], earlier: page.next_cursor || null });
        if (!gone) setError('');
      } catch (e) {
        if (!gone) setError((e as Error).message);
      } finally {
        if (!gone) setLoading(false);
      }
    }, FILTER_DEBOUNCE_MS);
    return () => {
      gone = true;
      clearTimeout(timer);
    };
  }, [unitRef, filter]);

  // Each snapshot also reads the lines after the last one, unless a filter narrows the
  // view or the lines came without cursors.
  useEffect(() => {
    const cursor = cursorOf(lines[lines.length - 1]);
    if (filter || !cursor || card.snapshotSeq === 0) return undefined;
    let gone = false;
    fetchLogs(unitRef, { cursor })
      .then((page) => {
        const items = page.items || [];
        if (!gone && items.length) setLines((old) => [...old, ...items].slice(-MAX_LINES));
      })
      .catch(() => {});
    return () => {
      gone = true;
    };
  }, [card.snapshotSeq]);

  const loadEarlier = async () => {
    if (!before) return;
    setLoading(true);
    try {
      const page = await fetchLogs(unitRef, {
        tail: PAGE,
        cursor: before,
        grep: filter || undefined
      });
      const items = page.items || [];
      if (found) setFound({ lines: [...items, ...found.lines], earlier: page.next_cursor || null });
      else {
        setLines((old) => [...items, ...old]);
        setEarlier(page.next_cursor || null);
      }
      setPicked((p) => (p == null ? p : p + items.length));
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setLoading(false);
    }
  };

  const line = picked == null ? null : shown[picked];
  const cursor = cursorOf(line);
  return (
    <>
      <div class="tools">
        <label class="search">
          <Icon name="search" />
          <span class="sr-only">Filter the logs of {unitRef}</span>
          <input
            type="search"
            placeholder="Filter lines"
            value={filter}
            onInput={(e) => setFilter(e.currentTarget.value)}
            spellcheck={false}
            autocomplete="off"
          />
        </label>
        <Button
          label={loading ? 'Loading…' : 'Load earlier'}
          disabled={!before || loading}
          onClick={loadEarlier}
        />
      </div>
      {shown.length ? (
        <Excerpt
          lines={shown}
          picked={picked}
          onPick={(i) => setPicked(i === picked ? null : i)}
          label={`logs of ${unitRef}`}
        />
      ) : (
        <Line>{loading ? 'Loading…' : filter ? 'No line matches.' : 'No lines.'}</Line>
      )}
      {error && <Line tone="danger">{error}</Line>}
      <Actions summary={line ? `${cursor || 'One line'} selected` : undefined}>
        {restart}
        {canHandOff() && (
          <Button
            key="ask"
            label={cursor ? `Ask agent about ${cursor}` : 'Ask agent about this line'}
            kind="primary"
            disabled={!line}
            onClick={() => line && ask(`${unitRef} logged: ${line.text.trim()}. Why?`)}
          />
        )}
      </Actions>
    </>
  );
}
