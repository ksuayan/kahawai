import { mount } from "@vue/test-utils";
import { describe, expect, it } from "vitest";
import UiBadge from "./UiBadge.vue";
import UiButton from "./UiButton.vue";
import { cn } from "./cn";

describe("UiButton", () => {
  it("is a type=button (never submits a form by accident)", () => {
    expect(mount(UiButton).attributes("type")).toBe("button");
  });

  it("renders its slot and forwards attributes and listeners", async () => {
    let clicked = 0;
    const w = mount(UiButton, {
      attrs: { title: "Scan", "aria-label": "Scan library", onClick: () => clicked++ },
      slots: { default: "Scan library" },
    });
    expect(w.text()).toBe("Scan library");
    expect(w.attributes("title")).toBe("Scan");
    expect(w.attributes("aria-label")).toBe("Scan library");
    await w.trigger("click");
    expect(clicked).toBe(1);
  });

  it("does not fire clicks when disabled", async () => {
    let clicked = 0;
    const w = mount(UiButton, { attrs: { disabled: true, onClick: () => clicked++ } });
    await w.trigger("click");
    expect(clicked).toBe(0);
    expect(w.attributes("disabled")).toBeDefined();
  });

  it("exposes the variant and active state as data attributes", () => {
    const w = mount(UiButton, { props: { variant: "primary", active: true } });
    expect(w.attributes("data-variant")).toBe("primary");
    expect(w.attributes("data-active")).toBeDefined();
    expect(mount(UiButton).attributes("data-active")).toBeUndefined();
  });

  it("styles each variant differently", () => {
    const cls = (v: string) => mount(UiButton, { props: { variant: v as never } }).classes().join(" ");
    expect(cls("primary")).toContain("bg-accent");
    expect(cls("danger")).toContain("text-danger");
    expect(cls("default")).not.toContain("bg-accent");
    expect(new Set(["default", "primary", "danger", "ghost", "icon", "nav"].map(cls)).size).toBe(6);
  });
});

describe("UiBadge", () => {
  it("renders its text with a variant", () => {
    const w = mount(UiBadge, { props: { variant: "ok" }, slots: { default: "done" } });
    expect(w.text()).toBe("done");
    expect(w.attributes("data-variant")).toBe("ok");
    expect(w.classes().join(" ")).toContain("text-ok");
  });
});

describe("cn", () => {
  it("joins strings, drops falsy values, and expands arrays and objects", () => {
    expect(cn("a", false, null, undefined, "b")).toBe("a b");
    expect(cn(["x", ["y", false]], { z: true, w: false })).toBe("x y z");
    expect(cn()).toBe("");
  });
});
