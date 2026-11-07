/** A CPU sparkline, scaled to its own peak; nothing with fewer than two samples. */
export function Spark({ values, label }: { values: number[]; label?: string }) {
  const last = values[values.length - 1];
  if (values.length < 2 || last === undefined) return null;
  const width = 56;
  const height = 14;
  const max = Math.max(...values, 1);
  const points = values
    .map(
      (v, i) =>
        `${((i / (values.length - 1)) * width).toFixed(1)},${(height - 1 - (v / max) * (height - 2)).toFixed(1)}`
    )
    .join(' ');
  return (
    <svg
      class="spark"
      width={width}
      height={height}
      viewBox={`0 0 ${width} ${height}`}
      role="img"
      aria-label={`${label || 'cpu'} ${last.toFixed(1)}%`}
    >
      <polyline points={points} />
    </svg>
  );
}
