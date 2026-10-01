import { beforeEach, describe, expect, it } from "vitest";
import { $$, mountApp, settle } from "../test/helpers";
import { tauri } from "../test/tauri-mock";
import { useDspStore } from "../stores/dsp";
import EqImportDialog from "./EqImportDialog.vue";

const TEXT = "Preamp: -6.2 dB\nFilter 1: ON PK Fc 100 Hz Gain 3 dB Q 1\nFilter 2: ON HSC Fc 9000 Hz Gain -2 dB Q 0.7";

beforeEach(() => {
  localStorage.clear();
  document.body.innerHTML = "";
});

async function open() {
  tauri.on("get_dsp_settings", { eq_bands: [], eq_enabled: true, loudness_enabled: false, loudness_target: -14 });
  const { wrapper } = mountApp(EqImportDialog, { open: true });
  await settle();
  return { wrapper, dsp: useDspStore() };
}
async function type(testid: string, value: string) {
  const el = $$(`[data-testid="${testid}"]`)[0] as HTMLInputElement | HTMLTextAreaElement;
  el.value = value;
  el.dispatchEvent(new Event("input", { bubbles: true }));
  await settle();
}

describe("EqImportDialog", () => {
  it("previews the profile, then replaces the EQ and saves a preset", async () => {
    const { dsp } = await open();
    expect($$('[data-testid="eq-import-apply"]')[0].hasAttribute("disabled")).toBe(true);
    await type("eq-import-text", TEXT);
    expect($$('[data-testid="eq-import-preview"]')[0].textContent).toContain("2 filters, preamp -6.2 dB");
    await type("eq-import-name", "HD 600");
    $$('[data-testid="eq-import-apply"]')[0].click();
    await settle();
    expect(dsp.rows).toHaveLength(2);
    expect(dsp.eqPreamp).toBe(-6.2);
    expect(dsp.userPresets.map((p) => p.name)).toEqual(["HD 600"]);
    expect(tauri.callsTo("set_eq_preamp").at(-1)).toEqual({ db: -6.2 });
  });

  it("shows warnings and rejects text with no filters", async () => {
    await open();
    await type("eq-import-text", `${TEXT}\nFilter 3: ON NOTCH Fc 50 Hz`);
    expect($$('[data-testid="eq-import-warning"]')).toHaveLength(1);
    await type("eq-import-text", "nonsense");
    expect($$('[data-testid="eq-import-invalid"]')).toHaveLength(1);
    expect($$('[data-testid="eq-import-apply"]')[0].hasAttribute("disabled")).toBe(true);
  });
});
