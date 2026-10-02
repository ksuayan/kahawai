import { describe, expect, it, vi } from "vitest";
import { SEEK_STEP_MS, VOLUME_STEP, handleShortcut, ownsKeyboard, type ShortcutActions } from "./shortcuts";

function actions(): ShortcutActions & Record<keyof ShortcutActions, ReturnType<typeof vi.fn>> {
  return { toggle: vi.fn(), seekBy: vi.fn(), volumeBy: vi.fn(), next: vi.fn(), prev: vi.fn(), go: vi.fn() } as never;
}

function fire(key: string, target: Element = document.body, init: KeyboardEventInit = {}) {
  const e = new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true, ...init });
  Object.defineProperty(e, "target", { value: target });
  return e;
}

function el(html: string): HTMLElement {
  const host = document.createElement("div");
  host.innerHTML = html;
  document.body.appendChild(host);
  return host.firstElementChild as HTMLElement;
}

describe("global shortcuts", () => {
  it("maps transport keys and prevents the browser default where it would scroll or click", () => {
    const a = actions();
    let e = fire(" ");
    expect(handleShortcut(e, a)).toBe(true);
    expect(a.toggle).toHaveBeenCalledOnce();
    expect(e.defaultPrevented).toBe(true);

    e = fire("ArrowRight");
    handleShortcut(e, a);
    expect(a.seekBy).toHaveBeenLastCalledWith(SEEK_STEP_MS);
    e = fire("ArrowLeft");
    handleShortcut(e, a);
    expect(a.seekBy).toHaveBeenLastCalledWith(-SEEK_STEP_MS);
    handleShortcut(fire("ArrowUp"), a);
    expect(a.volumeBy).toHaveBeenLastCalledWith(VOLUME_STEP);
    handleShortcut(fire("ArrowDown"), a);
    expect(a.volumeBy).toHaveBeenLastCalledWith(-VOLUME_STEP);

    handleShortcut(fire("n"), a);
    handleShortcut(fire("N"), a);
    handleShortcut(fire("p"), a);
    expect(a.next).toHaveBeenCalledTimes(2);
    expect(a.prev).toHaveBeenCalledOnce();
  });

  it("navigates with 1-6 and F", () => {
    const a = actions();
    for (const [k, v] of [["1", "albums"], ["2", "artists"], ["3", "playlists"], ["4", "search"], ["5", "queue"], ["6", "settings"], ["f", "search"], ["F", "search"], ["g", "genres"]]) {
      handleShortcut(fire(k), a);
      expect(a.go).toHaveBeenLastCalledWith(v);
    }
  });

  it("ignores unknown keys and modified keys (Cmd/Ctrl/Alt)", () => {
    const a = actions();
    expect(handleShortcut(fire("x"), a)).toBe(false);
    for (const mod of [{ metaKey: true }, { ctrlKey: true }, { altKey: true }]) {
      expect(handleShortcut(fire("n", document.body, mod), a)).toBe(false);
      expect(handleShortcut(fire(" ", document.body, mod), a)).toBe(false);
    }
    expect(a.next).not.toHaveBeenCalled();
    expect(a.toggle).not.toHaveBeenCalled();
  });

  it("stays out of the way while typing in a text field", () => {
    const a = actions();
    const input = el('<input type="text" />');
    expect(handleShortcut(fire("n", input), a)).toBe(false);
    expect(handleShortcut(fire("1", input), a)).toBe(false);
    expect(handleShortcut(fire(" ", input), a)).toBe(false);
    const textarea = el("<textarea></textarea>");
    expect(handleShortcut(fire("p", textarea), a)).toBe(false);
    expect(a.go).not.toHaveBeenCalled();
    expect(a.next).not.toHaveBeenCalled();
  });

  it("Escape blurs a focused input", () => {
    const a = actions();
    const input = el('<input type="text" />') as HTMLInputElement;
    input.focus();
    expect(document.activeElement).toBe(input);
    expect(handleShortcut(fire("Escape", input), a)).toBe(true);
    expect(document.activeElement).not.toBe(input);
  });
});

describe("analog A/B keys", () => {
  it("A and B pick a slot and X switches, in either case", () => {
    const ab = vi.fn();
    const a = { ...actions(), ab } as ShortcutActions;
    for (const [k, want] of [["a", "a"], ["A", "a"], ["b", "b"], ["B", "b"], ["x", "toggle"], ["X", "toggle"]] as const) {
      expect(handleShortcut(fire(k), a)).toBe(true);
      expect(ab).toHaveBeenLastCalledWith(want);
    }
  });

  it("is not handled without an A/B action, when modified, or while typing", () => {
    expect(handleShortcut(fire("x"), actions())).toBe(false);
    const ab = vi.fn();
    const a = { ...actions(), ab } as ShortcutActions;
    expect(handleShortcut(fire("x", document.body, { ctrlKey: true }), a)).toBe(false);
    expect(handleShortcut(fire("b", el("<input>")), a)).toBe(false);
    expect(ab).not.toHaveBeenCalled();
  });
});

describe("Reka widgets own their keys (regression: one keypress must not do two things)", () => {
  it.each([
    ['<div role="slider" tabindex="0"></div>', "ArrowRight"],
    ['<div role="slider" tabindex="0"></div>', "ArrowUp"],
    ['<button role="combobox">Auto</button>', "ArrowDown"],
    ['<div role="option">x</div>', "n"],
    ['<div role="listbox"><div role="option" id="o">x</div></div>', "p"],
    ['<div role="menu"><div role="menuitem" id="m">x</div></div>', "ArrowDown"],
    ['<div role="dialog"><button id="b">ok</button></div>', " "],
  ])("ignores %s + %s", (html, key) => {
    const a = actions();
    const root = el(html);
    const target = root.querySelector("#o, #m, #b") ?? root;
    expect(ownsKeyboard(target)).toBe(true);
    const e = fire(key, target);
    expect(handleShortcut(e, a)).toBe(false);
    expect(e.defaultPrevented).toBe(false);
    for (const fn of Object.values(a)) expect(fn).not.toHaveBeenCalled();
  });

  it("does not steal Space from a focused button or switch (they activate natively)", () => {
    const a = actions();
    const button = el("<button>Save</button>");
    const sw = el('<button role="switch" aria-checked="false"></button>');
    expect(handleShortcut(fire(" ", button), a)).toBe(false);
    expect(handleShortcut(fire(" ", sw), a)).toBe(false);
    expect(a.toggle).not.toHaveBeenCalled();
    // ...but other shortcuts still work with a button focused.
    expect(handleShortcut(fire("n", button), a)).toBe(true);
  });

  it("owns nothing for ordinary page content", () => {
    expect(ownsKeyboard(el("<div><span>hi</span></div>"))).toBe(false);
    expect(ownsKeyboard(null)).toBe(false);
  });
});

describe("audiobook shortcuts", () => {
  it("7 opens Audiobooks", () => {
    const a = actions();
    handleShortcut(fire("7"), a);
    expect(a.go).toHaveBeenCalledWith("audiobooks");
  });
  it("j and l skip back and forward when the app can skip, and do nothing otherwise", () => {
    const a = { ...actions(), skip: vi.fn() } as unknown as ShortcutActions & { skip: ReturnType<typeof vi.fn> };
    expect(handleShortcut(fire("j"), a)).toBe(true);
    expect(a.skip).toHaveBeenLastCalledWith(-1);
    expect(handleShortcut(fire("l"), a)).toBe(true);
    expect(a.skip).toHaveBeenLastCalledWith(1);
    expect(handleShortcut(fire("j"), actions())).toBe(false);
  });
  it("leaves j and l alone while typing", () => {
    const a = { ...actions(), skip: vi.fn() } as unknown as ShortcutActions & { skip: ReturnType<typeof vi.fn> };
    const input = el('<input type="text">');
    expect(handleShortcut(fire("j", input), a)).toBe(false);
    expect(a.skip).not.toHaveBeenCalled();
  });
});
