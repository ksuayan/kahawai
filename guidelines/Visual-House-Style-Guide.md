# House Style Guide

Look-and-feel standard for new projects. App-agnostic: it describes the
house, not any single app. Reference implementations: **Koa Photo Library**
(dense pro tool, neutral media surround) and **Kahawai Player** (dark-first,
Reka UI primitives, Lucide icons, IBM Plex).

**In one line:** quiet, sharp, honest software. The chrome recedes; the
content leads; the copy tells the truth.

---

## Principles

1. **Content over chrome.** The UI is a frame, not the picture. Borders are
   hairlines, fills are flat, decoration is near zero.
2. **Neutrality where judgment happens.** In media apps (Koa), surrounding
   grays are true neutrals — the UI must never tint color or tone judgment.
3. **Restraint is the brand.** One accent, two font weights, three radii.
   Limits are the style.
4. **Honest copy.** The interface says what happened and what to do next.
   No cheer, no blame, no jargon.
5. **Fast is beautiful.** Motion is functional; nothing decorative ever
   delays the user.

---

## Typography

Three families, two weights each. That's the whole system.

| Role | Family | Weights |
|---|---|---|
| UI chrome — nav, buttons, labels, tables, menus, headings | IBM Plex Sans | 400 / 600 |
| Body text, long-form prose, rich text | IBM Plex Serif | 400 / 600 |
| Code, hashes, IDs, technical readouts | IBM Plex Mono | 400 / 600 |

Bundle via Fontsource (`@fontsource/ibm-plex-sans` etc.), self-hosted
WOFF2. No CDN fonts. Hierarchy comes from size and spacing, not weight
variety — SemiBold is for headings and emphasis; everything else is
Regular.

**Scale (desktop px):**

| Token | Size / family | Use |
|---|---|---|
| Micro label | 11px Sans 600, uppercase, +0.08em tracking | section labels, eyebrows, table headers |
| UI base | 13px Sans 400 | default control/label text |
| UI large | 14px Sans 400 | primary content in lists |
| Prose | 16–17px Serif 400, 1.6 line-height | articles, descriptions, empty states |
| H3 / H2 / H1 | 15 / 18 / 24px Sans 600, −0.01em tracking | headings, sentence case |

Italics: avoid in UI chrome; acceptable sparingly in long-form serif
prose. Never fake-bold or fake-italic — if the weight isn't loaded, don't
use it.

---

## Color

**Dark-first.** Every project ships a dark theme; light theme is supported
where cheap (same token roles, inverted values), not as a second design.

**True-neutral grays.** No blue/purple tint in the ramp — critical for Koa,
harmless everywhere else.

Dark theme tokens:

| Token | Value | Use |
|---|---|---|
| `--bg` | `#0e0e10` | app background, media viewer surround |
| `--bg-raised` | `#161618` | sidebars, raised regions |
| `--surface` | `#1c1c1f` | cards, inputs, popovers |
| `--border` | `#2a2a2e` | hairline borders, dividers |
| `--text-1` | `#f2f2f3` | primary text |
| `--text-2` | `#a8a8ae` | secondary text |
| `--text-3` | `#6f6f77` | muted, placeholders, captions |
| `--accent` | per project | one hue — interactive, focus, selection |
| `--success` / `--warn` / `--error` | muted, not neon | semantic states only |

Light theme: `--bg: #fafafa`, `--surface: #ffffff`,
`--border: #e3e3e6`, `--text-1: #18181b`, `--text-2: #52525b`,
`--text-3: #8e8e96`. Same accent hue, adjusted for contrast.

**Accent rules (one signature accent, used sparingly):**

- Allowed: links, primary buttons, focus rings, active/selected states,
  progress, toggles-on.
- Forbidden: large fills, decorative gradients, accent-colored body text,
  more than one accent hue per project.
- The accent must pass 4.5:1 against `--bg` for text-sized usage; if it
  doesn't, use it only for large/bold elements and pair text with
  `--text-1`.

Semantic colors appear only where meaning is conveyed (status, validation).
They are never decorative.

---

## Shape

Sharp and flat — the pro-tool look.

- **Radii:** 2px (tags, badges, checkboxes), 4px (buttons, inputs, menus),
  6px (cards, dialogs). No fully-rounded pills except chips/tags where
  conventional.
- **Borders:** 1px hairlines (`--border`) do the separation work. Cards
  don't need shadows.
- **Shadows:** almost none. One restrained shadow is permitted on floating
  layers (dialogs, dropdowns, toasts) purely for figure/ground separation —
  e.g. `0 8px 24px rgb(0 0 0 / 0.35)`.

---

## Density & spacing

Base unit 4px. Two densities, chosen per surface:

| | Compact (pro surfaces: grids, tables, editors) | Comfortable (onboarding, settings, dialogs) |
|---|---|---|
| Control height | 28–32px | 36–40px |
| Row height | 32px | 44px+ |
| Gaps | 8px | 12–16px |
| Prose measure | — | max ~65ch |

Default to compact where the user works all day (Koa's grid, Kahawai's
queue); switch to comfortable where the user reads or decides (settings,
first-run, empty states). Never mix densities within one surface.

---

## Motion

Restrained expression — motion may have character, but it never delays.

- Durations: 120–180ms for fades/slides; 250ms max for large transitions.
  `ease-out` for entrances, `ease-in` for exits.
- Animate `opacity` and `transform` only; never layout properties.
- State changes (dialog open/close) key off component state attributes
  (e.g. Reka's `data-state`), not JS timeouts.
- Honor `prefers-reduced-motion`: disable non-essential animation
  entirely.

---

## Iconography

Lucide exclusively. Default 2px stroke; 14px inline with text, 16px in
buttons, 18–20px for feature/empty-state illustration. Icon-only buttons
get a 28–32px hit area and a `title`/`aria-label` — no exceptions. No emoji
in UI chrome, ever.

---

## Voice

Plain, dry, honest — everywhere, including marketing surfaces.

- Say what happened and what to do: "Sync failed: server unreachable.
  Check the LAN connection and retry." Not: "Oops! Something went wrong!"
- Sentence case for headings and buttons. No exclamation marks in UI copy.
- Errors name the cause when known, the next step always. Never blame the
  user; never fake certainty ("might have", "try" are fine when true).
- Empty states explain the situation and offer the action:
  "No playlists yet. Create one to get started."
- Units, numbers, and technical terms are exact. If a value is approximate,
  say so.

---

## Layout & composition

- **Chrome recedes.** Navigation and toolbars are quiet; the working area
  gets the pixels. In media apps the viewer surround is pure `--bg` —
  nothing near the image competes with it.
- **Lists and tables:** hairline row dividers, no zebra striping, right-
  aligned numerals, truncated text with tooltips over wrapped text.
- **Hierarchy through spacing**, not boxes-in-boxes. If a card needs a
  border *and* a shadow *and* a tinted fill, remove two of them.

---

## Accessibility (non-negotiable)

- Text contrast ≥ 4.5:1 (`--text-2` and `--text-3` are checked against
  `--bg` at their intended sizes).
- Visible focus: 2px accent outline, 2px offset, on every interactive
  element. Never remove outlines without replacing them.
- All actions keyboard-reachable; custom controls expose ARIA roles/states
  (Reka primitives handle this — don't rebuild them by hand).
- Icon-only controls are labeled; status is never color-alone (pair with
  text or icon).

---

## Implementation notes

- Tokens are CSS custom properties (`--bg`, `--surface`, `--border`,
  `--text-1/2/3`, `--accent`, `--radius-*`, `--font-sans/serif/mono`).
  Light theme is a `[data-theme="light"]` override of the same roles.
- In Tailwind projects, map tokens via the theme config — components
  reference tokens, never raw hex.
- This guide is visual and verbal language only. It doesn't dictate
  framework, component APIs, or file structure.

---

## Non-goals

- Not a component library spec — build components per project, in the
  project's idiom (Reka UI for Vue, etc.).
- Not a logo/brand-identity guide — the per-project accent and wordmark
  live with the project.
- Doesn't cover marketing-site heroics. If a landing page needs to shout,
  it borrows the voice and type, not the restraint.

---

## Reference points

- **Kahawai Player** — the full system in production: Plex Sans UI type,
  dark-first tokens, hairline-borders-and-flat-surfaces shape language,
  Lucide throughout, compact density in queue/library, controlled Reka
  dialogs driven by Pinia state.
- **Koa Photo Library** — true-neutral surround for color judgment, dense
  grid/list browsing, comfortable density reserved for settings and
  onboarding flows.
