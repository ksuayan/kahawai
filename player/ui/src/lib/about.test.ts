import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { aboutSourceFor, renderAbout, renderMarkdown } from "./about";

describe("About content", () => {
  it("fills in the version and renders headings, lists, tables and code", () => {
    const html = renderAbout("about", "1.2.3");
    expect(html).toContain("<h1>Kahawai Player</h1>");
    expect(html).toContain("Version 1.2.3");
    expect(html).not.toContain("{{version}}");
    expect(html).toContain("<h2>What it does</h2>");
    expect(html).toContain("<table>");
    expect(html).toContain("<li>");
  });

  it("covers what an About page should: privacy, copyright, trademarks, fonts and open source", () => {
    const text = aboutSourceFor("about", "1.0.0");
    for (const heading of ["Your music and your privacy", "Copyright and trademarks", "Fonts", "Open-source software", "Models and standards"]) {
      expect(text).toContain(`## ${heading}`);
    }
    expect(text).toContain("Copyright (c) 2026");
    expect(text).toContain("IBM Plex Sans");
    expect(text).toContain("IBM Plex Serif");
    expect(text).toMatch(/no authentication/i);
    expect(text).toContain("Koren");
  });

  it("has open-source notices with the font licence, npm and Rust tables, and the weak-copyleft note", () => {
    const html = renderAbout("notices", "1.0.0");
    expect(html).toContain("<h1>Open-source notices</h1>");
    expect(html).toContain("SIL OPEN FONT LICENSE Version 1.1");
    expect(html).toContain("JavaScript / TypeScript components");
    expect(html).toContain("Rust components");
    expect(html).toContain("marked");
    expect(html).toContain("symphonia");
    expect(html).toContain("weak-copyleft");
  });

  it("lists every direct dependency the UI ships (notices stay in step with package.json)", () => {
    const pkg = JSON.parse(readFileSync(join(__dirname, "../../package.json"), "utf8")) as { dependencies: Record<string, string> };
    const notices = aboutSourceFor("notices", "x");
    for (const name of Object.keys(pkg.dependencies)) expect(notices, name).toContain(`| ${name} |`);
  });

  it("shows links as text with the address, never as anchors, and escapes raw HTML", () => {
    const html = renderMarkdown('See [the docs](https://example.com/x) and <script>alert(1)</script> and <b>bold?</b>.');
    expect(html).not.toContain("<a ");
    expect(html).toContain("the docs (https://example.com/x)");
    expect(html).not.toContain("<script>");
    expect(html).toContain("&lt;script&gt;");
    expect(html).not.toContain("<b>");
  });
});

describe("disclaimers", () => {
  it("the README carries DISCLAIMER.md word for word (the About pages render the file itself)", async () => {
    const readme = (await import("../../../../README.md?raw")).default as string;
    const disclaimer = (await import("../../../../DISCLAIMER.md?raw")).default as string;
    expect(disclaimer).toContain("## Disclaimers");
    expect(readme).toContain(disclaimer.trim());
  });
});

