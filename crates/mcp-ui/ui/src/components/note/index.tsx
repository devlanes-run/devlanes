import type { NoteState } from '../../store/card';

/** A one-line note under a card: an error, or what a button did. */
export function Note({ note }: { note: NoteState | null }) {
  if (!note) return null;
  return (
    <p class={`note tone-${note.tone || 'quiet'}`} role="status">
      {note.text}
    </p>
  );
}
