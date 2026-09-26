import { Marked, type Tokens } from "marked";
import aboutSource from "../content/about.md?raw";
import noticesSource from "../content/notices.md?raw";

/**
 * Render the bundled Markdown for the About dialog.
 *
 * Links are shown as plain text with the address beside them: the window has no
 * permission to open external addresses (least-privilege capability), and a bare
 * anchor would navigate the app's own webview away from the player.
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

export type AboutPage = "about" | "notices";

/** Bundled source text for a page, with `{{version}}` filled in. */
export function aboutSourceFor(page: AboutPage, version: string): string {
  return (page === "about" ? aboutSource : noticesSource).replaceAll("{{version}}", version);
}

/** HTML for a Markdown string (links neutralized, raw HTML escaped). */
export function renderMarkdown(source: string): string {
  return md.parse(source, { async: false }) as string;
}

export function renderAbout(page: AboutPage, version: string): string {
  return renderMarkdown(aboutSourceFor(page, version));
}
