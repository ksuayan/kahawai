import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it } from "vitest";
import { tauri } from "../test/tauri-mock";
import { guessingChance, MATCH_TOLERANCE_DB, UNMATCH_TOLERANCE_DB, useAbxStore } from "./abx";
import { useAnalogStore } from "./analog";

const sent = () => tauri.callsTo("set_analog").map((c) => (c as { settings: { flavour: string; enabled: boolean } }).settings);

/** A sequence of "random" numbers: <0.5 makes X = A, otherwise X = B. */
const seq = (...v: number[]) => {
  let i = 0;
  return () => v[i++ % v.length];
};

beforeEach(() => {
  localStorage.clear();
  setActivePinia(createPinia());
});

function ready() {
  const analog = useAnalogStore();
  const abx = useAbxStore();
  analog.update("a", { enabled: true, flavour: "jfet" });
  analog.update("b", { enabled: true, flavour: "tube_300b" });
  analog.measured.a = 1.0;
  analog.measured.b = 1.2;
  tauri.calls.length = 0;
  return { analog, abx };
}

describe("guessing chance", () => {
  it("is the one-sided binomial tail at p = 0.5", () => {
    expect(guessingChance(10, 10)).toBeCloseTo(1 / 1024, 6);
    expect(guessingChance(10, 9)).toBeCloseTo(11 / 1024, 6);
    expect(guessingChance(10, 8)).toBeCloseTo(56 / 1024, 6);
    expect(guessingChance(10, 5)).toBeCloseTo(638 / 1024, 6);
    expect(guessingChance(10, 0)).toBe(1);
    expect(guessingChance(10, 11)).toBe(0);
    expect(guessingChance(1, 1)).toBe(0.5);
  });
});

describe("ABX blind test", () => {
  it("knows when the test would be unfair or meaningless", () => {
    const { analog, abx } = ready();
    expect(abx.slotsDiffer).toBe(true);
    expect(abx.levelMatched).toBe(true);
    expect(abx.levelDifference).toBeCloseTo(0.2, 5);
    analog.measured.b = 1.0 + UNMATCH_TOLERANCE_DB + 0.1;
    expect(abx.levelMatched).toBe(false);
    analog.measured.b = null;
    expect(abx.levelDifference).toBeNull();
    expect(abx.levelMatched).toBe(false);
    analog.copy("a", "b");
    expect(abx.slotsDiffer).toBe(false);
  });

  it("doesn't flicker near the tolerance: matched at 0.5 dB, unmatched only above 0.7 dB", () => {
    const { analog, abx } = ready(); // 0.2 dB apart: matched
    const at = (diff: number) => {
      analog.measured.b = (analog.measured.a ?? 0) + diff;
      return abx.levelMatched;
    };
    expect([at(0.45), at(0.6), at(0.55), at(0.69)]).toEqual([true, true, true, true]); // stays matched
    expect(at(0.75)).toBe(false);
    expect([at(0.6), at(0.55), at(0.69)]).toEqual([false, false, false]); // stays unmatched
    expect(at(0.5)).toBe(true);
  });

  it("hearing X plays the hidden slot, while the UI only ever sees 'x'", () => {
    const { abx } = ready();
    abx.start(3, seq(0.9)); // X is B
    expect(abx.running).toBe(true);
    expect(abx.heard).toBe("a");
    expect(sent().at(-1)).toMatchObject({ flavour: "jfet" });
    abx.hear("x");
    expect(abx.heard).toBe("x");
    expect(sent().at(-1)).toMatchObject({ flavour: "tube_300b" }); // the engine gets B
    abx.hear("a");
    expect(sent().at(-1)).toMatchObject({ flavour: "jfet" });
    expect(abx.revealed).toEqual([]); // nothing to see mid-test
  });

  it("scores a run, moves on after each answer, and reveals the truth only at the end", () => {
    const { abx } = ready();
    abx.start(4, seq(0.1, 0.9, 0.1, 0.9)); // X = A, B, A, B
    abx.answer("a"); // right
    expect(abx.answered).toBe(1);
    expect(abx.heard).toBe("x"); // stays on X for the next trial
    expect(abx.revealed).toEqual([]);
    abx.answer("a"); // wrong (X was B)
    abx.answer("a"); // right
    expect(abx.running).toBe(true);
    abx.answer("b"); // right: finishes
    expect(abx.running).toBe(false);
    expect(abx.finished).toBe(true);
    expect(abx.correct).toBe(3);
    expect(abx.chance).toBeCloseTo(guessingChance(4, 3), 6);
    expect(abx.revealed.map((t) => `${t.truth}${t.guess}`)).toEqual(["aa", "ba", "aa", "bb"]);
    expect(abx.verdict).toContain("3 of 4");
    expect(abx.verdict).toContain("cannot show"); // 4 trials are too few to prove anything at 3/4
  });

  it("says you can hear the difference when the score is unlikely by chance", () => {
    const { abx } = ready();
    abx.start(10, seq(0.1, 0.9));
    for (let i = 0; i < 10; i++) abx.answer(i % 2 === 0 ? "a" : "b"); // all right
    expect(abx.correct).toBe(10);
    expect(abx.verdict).toContain("you can hear the difference");
    expect(abx.verdict).toContain("0.10%");
  });

  it("does nothing outside a test, and cancelling returns to A with no result", () => {
    const { analog, abx } = ready();
    abx.hear("x");
    abx.answer("a");
    expect(abx.answered).toBe(0);
    abx.start(5, seq(0.9));
    abx.hear("x");
    abx.answer("b");
    abx.cancel();
    expect(abx.running).toBe(false);
    expect(abx.finished).toBe(false);
    expect(abx.answered).toBe(0);
    expect(analog.active).toBe("a");
    abx.start(1, seq(0.1));
    abx.answer("a");
    abx.dismiss();
    expect(abx.finished).toBe(false);
    expect(abx.revealed).toEqual([]);
  });

  it("mixes X between A and B with a fair coin (roughly half each) using the real random source", () => {
    const { abx } = ready();
    abx.start(20);
    // Answer "A" every time: about half should be right.
    for (let i = 0; i < 20; i++) abx.answer("a");
    expect(abx.correct).toBeGreaterThan(3);
    expect(abx.correct).toBeLessThan(17);
  });
});
