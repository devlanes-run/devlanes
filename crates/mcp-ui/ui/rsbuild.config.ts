import { defineConfig, type RsbuildPlugin } from '@rsbuild/core';
import { pluginPreact } from '@rsbuild/plugin-preact';
import { pluginSass } from '@rsbuild/plugin-sass';

/** The placeholder `src/index.html` holds where the policy goes. */
const CSP_PLACEHOLDER = '__CSP__';

/**
 * Base64 SHA-256 of `text`, the form a Content-Security-Policy hash source takes.
 * Web Crypto rather than `node:crypto`, so this file type-checks with the browser
 * libraries the rest of the package uses.
 */
async function sha256(text: string): Promise<string> {
  const digest = new Uint8Array(
    await crypto.subtle.digest('SHA-256', new TextEncoder().encode(text))
  );
  return btoa(String.fromCharCode(...digest));
}

/**
 * Where hosts serve the fonts their `styles.css.fonts` names: Claude's Anthropic Sans.
 * The only origin the card may load from; the request carries no devlanes data, and
 * the devlanes MCP server lists the same origin in `_meta.ui.csp.resourceDomains`.
 */
const FONT_ORIGINS = ['https://assets.claude.ai'];

/**
 * Fills the card's Content-Security-Policy with the hashes of its inline script
 * and stylesheet once rsbuild has inlined them, so the card runs no other code
 * or style and requests nothing but host fonts (`default-src 'none'`). Fails the
 * build on an external script, which the policy would block in the host anyway.
 */
function pluginCsp(): RsbuildPlugin {
  return {
    name: 'devlanes:csp',
    setup(api) {
      api.modifyHTML(async (html) => {
        const scripts = [...html.matchAll(/<script\b([^>]*)>([\s\S]*?)<\/script>/g)];
        const styles = [...html.matchAll(/<style\b[^>]*>([\s\S]*?)<\/style>/g)];
        if (
          scripts.length === 0 ||
          scripts.some(([, attributes]) => /\bsrc=/.test(attributes ?? ''))
        ) {
          throw new Error('the card must have its script inline');
        }
        // A `</script` inside the code would have ended its tag early.
        if ((html.match(/<script\b/g) ?? []).length !== scripts.length) {
          throw new Error('the card script must not contain </script');
        }
        if (!html.includes(CSP_PLACEHOLDER)) {
          throw new Error(`src/index.html has no ${CSP_PLACEHOLDER}`);
        }
        const sources = async (bodies: string[]) =>
          (await Promise.all(bodies.map(sha256))).map((hash) => `'sha256-${hash}'`).join(' ');
        const csp = [
          "default-src 'none'",
          `script-src ${await sources(scripts.map((match) => match[2] ?? ''))}`,
          `style-src ${await sources(styles.map((match) => match[1] ?? ''))}`,
          `font-src ${FONT_ORIGINS.join(' ')}`,
          "base-uri 'none'",
          "form-action 'none'"
        ].join('; ');
        return html.replace(CSP_PLACEHOLDER, csp);
      });
    }
  };
}

export default defineConfig({
  plugins: [pluginPreact({ reactAliasesEnabled: false }), pluginSass(), pluginCsp()],
  source: {
    entry: { lane: './src/index.tsx' }
  },
  html: {
    template: './src/index.html',
    title: 'devlanes',
    // The template writes these itself, before the policy.
    meta: { charset: false, viewport: false },
    // An inline script runs where it stands, so it goes after the root it renders into.
    inject: 'body',
    outputStructure: 'flat'
  },
  output: {
    // One HTML file with everything inside: hosts load the card as one resource.
    inlineScripts: true,
    inlineStyles: true,
    dataUriLimit: Number.MAX_SAFE_INTEGER,
    // `linked` would write a .LICENSE.txt next to the card; the README names the licenses.
    legalComments: 'none',
    // Chat hosts embed current Chromium, WebKit or Gecko; the stylesheet needs
    // container queries and :has() anyway, so older engines get no transpiling.
    overrideBrowserslist: ['chrome >= 111', 'edge >= 111', 'firefox >= 121', 'safari >= 16.4']
  },
  // rsbuild 2's form of `performance.chunkSplit: { strategy: 'all-in-one' }`.
  splitChunks: false
});
