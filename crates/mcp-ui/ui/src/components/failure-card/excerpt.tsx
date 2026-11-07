import type { LogLine } from '../../types/proto';

/** Lines an inline excerpt shows at most: enough for a short stack trace, no scrolling. */
const MAX_LINES = 8;

/**
 * The level of a line, from the daemon or, for plain lines a start result carries,
 * from its words.
 */
export function levelOf(line: LogLine): 'e' | 'w' | '' {
  const level = line.level || '';
  if (
    level === 'error' ||
    (!level && /\berror\b|exception|traceback|^\s+at |panicked/i.test(line.text))
  ) {
    return 'e';
  }
  if (level === 'warn' || (!level && /\bwarn(ing)?\b/i.test(line.text))) return 'w';
  return '';
}

/** The first line of the last run of error lines: the error itself, above its stack. */
export function lastError(lines: LogLine[]): LogLine | undefined {
  let index = -1;
  for (let i = lines.length - 1; i >= 0; i--) {
    const line = lines[i];
    if (line && levelOf(line) === 'e') index = i;
    else if (index >= 0) break;
  }
  return index >= 0 ? lines[index] : undefined;
}

/**
 * The lines that explain a failure: from just before the last error to the line after
 * its stack trace, so the trace stays whole, at most {@link MAX_LINES}. Without an error,
 * the last lines.
 */
export function excerptOf(lines: LogLine[]): LogLine[] {
  const error = lastError(lines);
  if (!error) return lines.slice(-MAX_LINES);
  const start = lines.indexOf(error);
  let end = start;
  while (end + 1 < lines.length && levelOf(lines[end + 1] as LogLine) === 'e') end++;
  const to = Math.min(lines.length, end + 2);
  const from = Math.max(0, Math.min(start - 2, to - MAX_LINES));
  return lines.slice(from, Math.min(to, from + MAX_LINES));
}

/** Log lines in a mono well; with `onPick`, each line is a button that picks it. */
export function Excerpt({
  lines,
  picked,
  onPick,
  label
}: {
  lines: LogLine[];
  picked?: number | null;
  onPick?: (index: number) => void;
  label: string;
}) {
  return (
    <div class="well" role="log" aria-label={label}>
      {lines.map((line, i) => {
        const cursor = line.seq != null ? `${line.unit}#${line.seq}` : null;
        const cls = `ln ${levelOf(line)}`;
        const body = (
          <>
            {cursor && <span class="cur">{cursor}</span>}
            <span class="tx">{line.text}</span>
          </>
        );
        return onPick ? (
          <button
            key={cursor ?? i}
            type="button"
            class={`${cls}${i === picked ? ' picked' : ''}`}
            aria-pressed={i === picked}
            onClick={() => onPick(i)}
          >
            {body}
          </button>
        ) : (
          <p key={cursor ?? i} class={cls}>
            <span class="tx">{line.text}</span>
          </p>
        );
      })}
    </div>
  );
}
