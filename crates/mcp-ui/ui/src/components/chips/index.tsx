/** The lane colour mark of the website's lane table. */
export function LaneMark({ index }: { index: number | null | undefined }) {
  return <span class={`lane-mark lane-${(index ?? 0) % 4}`} aria-hidden="true" />;
}

/** Actor badges: who started the units. */
export function Actors({ actors }: { actors: string[] }) {
  return (
    <>
      {actors.map((actor) => (
        <span key={actor} class={`actor${actor === 'you' ? ' you' : ''}`} title={actor}>
          {actor}
        </span>
      ))}
    </>
  );
}
