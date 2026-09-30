import { describe, expect, it } from "vitest";
import { installNativeMenuGuard } from "./nativeMenu";

function rightClick(el: Element): MouseEvent {
  const e = new MouseEvent("contextmenu", { bubbles: true, cancelable: true });
  el.dispatchEvent(e);
  return e;
}

describe("the WebKit menu guard", () => {
  function setup(allowed: boolean) {
    document.body.innerHTML = `<div id="plain">text</div><input id="field" /><div contenteditable="true" id="edit">x</div>`;
    const stop = installNativeMenuGuard(() => allowed);
    const $ = (id: string) => document.getElementById(id)!;
    return { stop, $ };
  }

  it("hides WebKit's menu where the app has none of its own", () => {
    const { stop, $ } = setup(false);
    expect(rightClick($("plain")).defaultPrevented).toBe(true);
    stop();
  });

  it("keeps it in text fields and on selected text (Copy, Paste)", () => {
    const { stop, $ } = setup(false);
    expect(rightClick($("field")).defaultPrevented).toBe(false);
    expect(rightClick($("edit")).defaultPrevented).toBe(false);
    const range = document.createRange();
    range.selectNodeContents($("plain"));
    document.getSelection()!.removeAllRanges();
    document.getSelection()!.addRange(range);
    expect(rightClick($("plain")).defaultPrevented).toBe(false);
    document.getSelection()!.removeAllRanges();
    stop();
  });

  it("lets it through everywhere when developer tools are on", () => {
    const { stop, $ } = setup(true);
    expect(rightClick($("plain")).defaultPrevented).toBe(false);
    stop();
  });
});
