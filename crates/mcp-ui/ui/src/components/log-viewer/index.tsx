// The log viewer of the failure card: lines with their cursors, grep, "load
// earlier" and a picked line the agent can read around.

import { useEffect, useLayoutEffect, useRef, useState } from 'preact/hooks';
import { act, ask } from '../../store/actions';
import { bridge, card, cwd, readOnly } from '../../store/card';
import type { LogLine, LogsPage } from '../../types/proto';
import type { LogsResult } from '../../types/results';
import { Button } from '../button';
import type { Subject } from '../failure-card/subjects';
import { Icon } from '../icon';
import { Note } from '../note';

/** Lines the viewer keeps; older ones drop off the top while it follows. */
const MAX_LINES = 2000;
/** Lines each fetch asks for. */
const PAGE = 200;
/** How long typing in grep settles before the viewer fetches. */
const GREP_DEBOUNCE_MS = 300;

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

/**
 * The log viewer. Lines carry their cursor (`api#118`); a click picks one, and
 * "send to agent" hands the agent that cursor instead of the whole log.
 */
export function LogViewer({ subject, unitRef }: { subject: Subject; unitRef: string }) {
  const fromResult: LogLine[] =
    subject.lines ||
    (subject.tail || []).map((text) => ({ unit: subject.unit, seq: null, text, level: null }));
  const [lines, setLines] = useState<LogLine[]>(fromResult);
  const [earlier, setEarlier] = useState<string | null>(subject.next || null);
  const [grep, setGrep] = useState('');
  const [picked, setPicked] = useState<number | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState('');
  const list = useRef<HTMLOListElement | null>(null);
  const stick = useRef(true);
  const first = useRef(true);

  useEffect(() => {
    // A logs result already holds the latest page; anything else fetches it.
    if (first.current && subject.lines && !grep) {
      first.current = false;
      return undefined;
    }
    first.current = false;
    let gone = false;
    const timer = setTimeout(
      async () => {
        setLoading(true);
        try {
          const page = await fetchLogs(unitRef, { tail: PAGE, grep: grep || undefined });
          if (gone) return;
          setLines(page.items || []);
          setEarlier(page.next_cursor || null);
          setPicked(null);
          setError('');
          stick.current = true;
        } catch (e) {
          if (!gone) setError((e as Error).message);
        } finally {
          if (!gone) setLoading(false);
        }
      },
      grep ? GREP_DEBOUNCE_MS : 0
    );
    return () => {
      gone = true;
      clearTimeout(timer);
    };
  }, [unitRef, grep]);

  // Each snapshot tick also reads the lines after the last one, unless a grep
  // narrows the view or the lines came without cursors.
  useEffect(() => {
    const cursor = cursorOf(lines[lines.length - 1]);
    if (grep || !cursor || card.snapshotSeq === 0) return undefined;
    let gone = false;
    fetchLogs(unitRef, { cursor })
      .then((page) => {
        const items = page.items || [];
        if (gone || !items.length) return;
        setLines((old) => [...old, ...items].slice(-MAX_LINES));
      })
      .catch(() => {});
    return () => {
      gone = true;
    };
  }, [card.snapshotSeq]);

  useLayoutEffect(() => {
    const el = list.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, [lines]);

  const loadEarlier = async () => {
    if (!earlier) return;
    setLoading(true);
    try {
      const page = await fetchLogs(unitRef, {
        tail: PAGE,
        cursor: earlier,
        grep: grep || undefined
      });
      const items = page.items || [];
      stick.current = false;
      setLines((old) => [...items, ...old]);
      setPicked((p) => (p == null ? p : p + items.length));
      setEarlier(page.next_cursor || null);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setLoading(false);
    }
  };

  const onScroll = () => {
    const el = list.current;
    if (el) stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 8;
  };

  const line = picked == null ? null : lines[picked];
  const cursor = cursorOf(line);
  const send = () => {
    if (!line) return;
    ask(
      cursor
        ? `Look at ${unitRef} around ${cursor}: devlanes logs ${unitRef} --around ${cursor} -C 10`
        : `Look at this line from ${unitRef}: ${line.text}`
    );
  };
  const busy = card.busy[unitRef];

  return (
    <div class="logs">
      <div class="toolbar">
        <label class="grep">
          <Icon name="grep" />
          <span class="sr-only">grep the logs of {unitRef}</span>
          <input
            type="search"
            placeholder="grep"
            value={grep}
            onInput={(e) => setGrep(e.currentTarget.value)}
            spellcheck={false}
            autocomplete="off"
          />
        </label>
        <Button label="load earlier" disabled={!earlier || loading} onClick={loadEarlier} />
      </div>
      <ol
        class="logview"
        ref={list}
        onScroll={onScroll}
        aria-label={`logs of ${unitRef}`}
        aria-busy={loading}
      >
        {lines.length === 0 && (
          <li class="dim">{loading ? 'loading…' : grep ? 'no line matches' : 'no lines'}</li>
        )}
        {lines.map((l, i) => (
          <li key={cursorOf(l) ?? i}>
            <button
              type="button"
              class={`line${i === picked ? ' picked' : ''}${l.level === 'error' ? ' lvl-error' : l.level === 'warn' ? ' lvl-warn' : ''}`}
              aria-pressed={i === picked}
              onClick={() => setPicked(i === picked ? null : i)}
            >
              {l.seq != null && <span class="cur">{cursorOf(l)}</span>}
              <span class="txt">{l.text}</span>
            </button>
          </li>
        ))}
      </ol>
      {error && (
        <p class="note tone-failure" role="status">
          {error}
        </p>
      )}
      <Note note={card.note} />
      <div class="row">
        <span class="dim">
          {line ? (
            <>
              picked <span class="url">{cursor || `line ${picked == null ? '' : picked + 1}`}</span>
            </>
          ) : (
            'click a line to pick it'
          )}
        </span>
        <span class="actions">
          {!readOnly() && (
            <Button
              icon="restart"
              label={`restart ${subject.unit}`}
              disabled={!!busy}
              onClick={() => act('restart', unitRef, `restarted ${unitRef}`)}
            />
          )}
          <Button
            icon="ask"
            label={cursor ? `send ${cursor} to agent` : 'send line to agent'}
            primary
            disabled={!line}
            onClick={send}
          />
        </span>
      </div>
    </div>
  );
}
