// Copies single-source assets into site/public before dev/build.
// Sources: assets/ (brand), docs/screenshots/ (captures), scripts/install.sh.
import { copyFileSync, mkdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const repo = join(here, '..');
const publicDir = join(here, 'public');

const copies = [
  ['assets/bootable-mark.svg', 'favicon.svg'],
  ['assets/bootable-logo.svg', 'bootable-logo.svg'],
  ['assets/bootable-logo-animated.svg', 'bootable-logo-animated.svg'],
  ['docs/screenshots/gui-discover.png', 'gui-discover.png'],
  ['docs/screenshots/gui-demo.gif', 'gui-demo.gif'],
  ['docs/screenshots/gui-toolbar.png', 'gui-toolbar.png'],
  ['docs/screenshots/omarchy-plugin.png', 'omarchy-plugin.png'],
  ['docs/screenshots/tui-main.png', 'tui-main.png'],
  ['docs/screenshots/tui-demo.gif', 'tui-demo.gif'],
  ['scripts/install.sh', 'install.sh'],
];

mkdirSync(publicDir, { recursive: true });
for (const [from, to] of copies) {
  copyFileSync(join(repo, from), join(publicDir, to));
}
