import { card } from '../../store/card';
import type { LaneSummary } from '../../types/proto';

/** Conditions that need a look without stopping the lane. */
export function Warnings({ lane }: { lane: LaneSummary }) {
  const lines: string[] = [];
  if (lane.needs_env)
    lines.push('the lane lost its environment; a start from a shell brings it back');
  if (lane.config_changed) {
    lines.push('devlanes.yaml changed since the lane started; reload applies it');
  }
  for (const warning of lane.warnings || []) lines.push(warning.data?.message || warning.code);
  if (card.cancelled) lines.push(`cancelled: ${card.cancelled}; services keep starting`);
  const power = lane.power && (
    <li class="dim">
      {lane.power.level} while idle; its processes keep their state and the next request or
      connection thaws it
    </li>
  );
  if (!lines.length && !power) return null;
  return (
    <ul class="warnings">
      {lines.map((line) => (
        <li key={line} class="tone-attention">
          ! {line}
        </li>
      ))}
      {power}
    </ul>
  );
}
