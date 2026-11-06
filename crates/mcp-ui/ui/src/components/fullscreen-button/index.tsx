import { flash } from '../../store/actions';
import { bridge, card, update } from '../../store/card';
import { Button } from '../button';

/** The fullscreen toggle, shown only where the host offers fullscreen. */
export function FullscreenButton() {
  const modes = card.context.availableDisplayModes || [];
  if (!modes.includes('fullscreen')) return null;
  const full = card.context.displayMode === 'fullscreen';
  const toggle = async () => {
    try {
      const mode = await bridge.requestDisplayMode(full ? 'inline' : 'fullscreen');
      update({ context: { ...card.context, displayMode: mode } });
      document.documentElement.classList.toggle('fullscreen', mode === 'fullscreen');
    } catch (error) {
      flash({ text: `no fullscreen: ${(error as Error).message}`, tone: 'failure' });
    }
  };
  return (
    <Button
      icon={full ? 'inline' : 'fullscreen'}
      label={full ? 'exit fullscreen' : 'fullscreen'}
      iconOnly
      onClick={toggle}
    />
  );
}
