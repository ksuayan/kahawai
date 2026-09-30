import { Marked, type Tokens } from "marked";
import aboutSource from "../content/about.md?raw";
import noticesSource from "../content/notices.md?raw";
// The repository's DISCLAIMER.md, at the bottom of the About page (the README
// carries the same text; a test keeps them identical).
import disclaimerSource from "../../../../../DISCLAIMER.md?raw";
// The repository's LICENSE (GNU AGPL v3), shown as-is in the License tab.
import licenseSource from "../../../../../LICENSE?raw";

/**
 * Render the bundled Markdown for the About dialog (same rules as the
 * player's src/lib/about.ts).
 *
 * Links are shown as plain text with the address beside them: the window has
 * no permission to open external addresses, and a bare anchor would navigate
 * the app's own webview away.
 */
const md = new Marked({
  gfm: true,
  renderer: {
    link({ href, tokens }: Tokens.Link): string {
      const text = this.parser.parseInline(tokens);
      return href && !text.includes(href) ? `${text} (${escapeHtml(href)})` : text;
    },
    // Raw HTML in the source is shown as text, never interpreted.
    html({ text }: Tokens.HTML | Tokens.Tag): string {
      return escapeHtml(text);
    },
  },
});

function escapeHtml(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
}

export type AboutPage = "about" | "notices" | "license";

/** The full license text (plain text, shown preformatted). */
export const licenseText: string = licenseSource;

export function renderAbout(page: AboutPage, version: string): string {
  const source = (page === "about" ? `${aboutSource}\n\n${disclaimerSource}` : noticesSource).replaceAll(
    "{{version}}",
    version,
  );
  return md.parse(source, { async: false }) as string;
}
