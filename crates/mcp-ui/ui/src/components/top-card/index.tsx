// The top card for `debug top`: the machine's CPU and memory in one line, then the
// running units and containers by CPU with a small bar each, grouped by lane when there
// are several, and one button that asks the agent about the busiest one.

import { useState } from 'preact/hooks';
import { ask } from '../../store/actions';
import { canHandOff, card, isFullscreen, structured } from '../../store/card';
import type { TopPage, TopRow } from '../../types/proto';
import { size } from '../../utils/format';
import { Button } from '../button';
import { Actions, Card, Fold, Head, Line, SHOWN } from '../card';

/** A CPU share as `0.4%`, `12%` or `230%`; a dash before the first sample. */
function percent(value: number | null | undefined): string {
  if (value == null) return '–';
  return `${value < 10 ? value.toFixed(1) : Math.round(value)}%`;
}

/** A row's CPU, with no sample yet counting as 0 for sorting and sums. */
function cpuOf(row: TopRow): number {
  return row.cpu_percent ?? 0;
}

/**
 * The tone of a bar: info, warning from half a core (or a tenth of the memory), danger
 * from a whole core (or a quarter), as the TUI's monitor turns red past one core.
 */
function tone(share: number, warn: number, danger: number): string {
  return share >= danger ? 'danger' : share >= warn ? 'warn' : 'info';
}

/** A 40 px bar filled to `share` (0-1) in a tone's text colour. */
function Bar({ share, tone: color }: { share: number; tone: string }) {
  return (
    <span class={`meter ${color}`} aria-hidden="true">
      <i style={{ width: `${Math.round(Math.min(1, Math.max(0, share)) * 100)}%` }} />
    </span>
  );
}

/** What a row is when it is no plain service: a container or an exec unit. */
function kindOf(row: TopRow): string {
  const role = row.kind === 'container' ? row.kind : row.role || row.kind;
  return role && role !== 'service' ? role : '';
}

/** A row at the start of a question: `acme@1:web`, or `The postgres container of acme (shared)`. */
function subjectOf(row: TopRow): string {
  return row.kind === 'container'
    ? `The ${row.unit} container of ${row.lane}`
    : `${row.lane}:${row.unit}`;
}

/** One lane's rows, busiest first, with their totals. */
interface Group {
  lane: string;
  rows: TopRow[];
  cpu: number;
  memory: number;
}

/** The running rows grouped by lane, the busiest lane first. */
function groupsOf(rows: TopRow[]): Group[] {
  const groups = new Map<string, Group>();
  for (const row of rows) {
    const group = groups.get(row.lane) || { lane: row.lane, rows: [], cpu: 0, memory: 0 };
    group.rows.push(row);
    group.cpu += cpuOf(row);
    group.memory += row.memory_bytes ?? 0;
    groups.set(row.lane, group);
  }
  const byUse = (a: TopRow, b: TopRow) =>
    cpuOf(b) - cpuOf(a) || (b.memory_bytes ?? 0) - (a.memory_bytes ?? 0);
  return [...groups.values()]
    .map((group) => ({ ...group, rows: group.rows.sort(byUse) }))
    .sort((a, b) => b.cpu - a.cpu || b.memory - a.memory);
}

/** The machine in one line: `CPU 23% of 10 cores · memory 11 of 32 GiB · pressure normal`. */
function systemLine(top: TopPage): string[] {
  const sys = top.system;
  if (!sys) return [];
  const total = sys.memory_total_bytes;
  const available = sys.available_memory_bytes;
  return [
    sys.cpu_percent != null &&
      `CPU ${percent(sys.cpu_percent)}${sys.cpus ? ` of ${sys.cpus} cores` : ''}`,
    total != null && available != null && `memory ${size(total - available)} of ${size(total)}`,
    sys.memory_pressure && `pressure ${sys.memory_pressure}`
  ].filter((part): part is string => !!part);
}

/** The top card. */
export function TopCard() {
  const [all, setAll] = useState(false);
  const top: TopPage = card.snapshot?.top || structured()?.data || {};
  // Ended rows keep a graph's past; this card is about what runs now.
  const running = (top.units || []).filter((row) => !row.ended && !row.removed);
  const groups = groupsOf(running);
  const grouped = groups.length > 1;
  const ordered = groups.flatMap((group) => group.rows);
  const folds = ordered.length > SHOWN && !isFullscreen();
  const shown = new Set(folds && !all ? ordered.slice(0, SHOWN) : ordered);
  const rest = ordered.filter((row) => !shown.has(row));
  const memoryTotal = top.system?.memory_total_bytes || 0;
  const memoryMax = Math.max(1, ...running.map((row) => row.memory_bytes ?? 0));
  const busiest = [...running].sort((a, b) => cpuOf(b) - cpuOf(a))[0];
  const visible = groups
    .map((group) => ({ ...group, rows: group.rows.filter((row) => shown.has(row)) }))
    .filter((group) => group.rows.length);

  const row = (item: TopRow) => {
    const memory = item.memory_bytes ?? 0;
    const memoryShare = memoryTotal ? memory / memoryTotal : memory / memoryMax;
    const kind = kindOf(item);
    return (
      <li class="row" key={`${item.lane}:${item.unit}`}>
        <span class="name">{item.unit}</span>
        <span class="grow meta">{kind}</span>
        <span class="col num use">
          <Bar share={cpuOf(item) / 100} tone={tone(cpuOf(item), 50, 100)} />
          <span class="n">{percent(item.cpu_percent)}</span>
        </span>
        <span class="end num use mem">
          <span class="w520">
            <Bar share={memoryShare} tone={memoryTotal ? tone(memoryShare, 0.1, 0.25) : 'info'} />
          </span>
          <span class="n">{item.memory_bytes == null ? '–' : size(item.memory_bytes)}</span>
        </span>
      </li>
    );
  };

  return (
    <Card label="CPU and memory">
      <Head
        title="CPU and memory"
        sub={!grouped ? groups[0]?.lane : undefined}
        meta={systemLine(top)}
      />
      {!running.length && <Line>No units or containers are running.</Line>}
      {visible.map((group, index) => (
        <section key={group.lane} aria-label={group.lane}>
          {grouped && (
            <p class="group">
              {group.lane} · CPU {percent(group.cpu)} · {size(group.memory)}
            </p>
          )}
          <ul class="rows">
            {group.rows.map(row)}
            {folds && index === visible.length - 1 && (
              <Fold
                open={all}
                text={`${rest.length} more: ${percent(rest.reduce((sum, r) => sum + cpuOf(r), 0))} CPU, ${size(rest.reduce((sum, r) => sum + (r.memory_bytes ?? 0), 0))} together`}
                onToggle={() => setAll(!all)}
              />
            )}
          </ul>
        </section>
      ))}
      <Actions>
        {busiest && canHandOff() && (
          <Button
            key="ask"
            label={`Ask agent about ${busiest.unit}`}
            kind="primary"
            onClick={() =>
              ask(
                `${subjectOf(busiest)} uses ${percent(busiest.cpu_percent)} CPU and ${size(busiest.memory_bytes) || 'unknown memory'}. Is that expected, and what is it doing?`
              )
            }
          />
        )}
      </Actions>
    </Card>
  );
}
