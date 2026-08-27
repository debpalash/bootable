import { defineConfig } from 'astro/config';
import { readdirSync, existsSync, renameSync, rmSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));

// Flattens /<name>/index.html to /<name>.html so public URLs, canonical
// links, and the sitemap keep the existing .html form.
const flattenHtmlPages = {
  name: 'bootable:flatten-html',
  hooks: {
    'astro:build:done': ({ dir }) => {
      const out = fileURLToPath(dir);
      const pages = readdirSync(join(here, 'src/pages'))
        .filter((f) => f.endsWith('.astro') && f !== 'index.astro' && f !== '404.astro')
        .map((f) => f.replace(/\.astro$/, ''));
      for (const name of pages) {
        const nested = join(out, name, 'index.html');
        if (existsSync(nested)) {
          renameSync(nested, join(out, `${name}.html`));
          rmSync(join(out, name), { recursive: true });
        }
      }
    },
  },
};

export default defineConfig({
  site: 'https://bootable.palash.dev',
  integrations: [flattenHtmlPages],
});
