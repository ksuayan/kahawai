import { defineStore } from "pinia";
import { computed, ref } from "vue";
import { useAnalogStore, type Slot } from "./analog";

/** Which of the three listening choices is playing: the two references, or the hidden X. */
export type Heard = "a" | "b" | "x";

export interface Trial {
  /** What X really was. Kept out of reach (see `revealed`) until the test is over. */
  truth: Slot;
  guess: Slot;
}

/** The two levels may differ by this much (dB) before a blind test is considered unfair. */
export const MATCH_TOLERANCE_DB = 0.5;

/** One-sided binomial probability of getting at least `k` of `n` right by guessing (p = 0.5). */
export function guessingChance(n: number, k: number): number {
  if (k <= 0) return 1;
  if (k > n) return 0;
  let total = 0;
  let c = 1; // C(n, 0)
  for (let i = 0; i <= n; i++) {
    if (i >= k) total += c;
    c = (c * (n - i)) / (i + 1);
  }
  return total / 2 ** n;
}

/**
 * ABX blind test for the analog A/B pair. A and B stay known; X is secretly
 * one of them, chosen at random each trial. Listen to A, B and X as often as
 * you like, then say which one X is. After all trials the score and the
 * chance of getting it by guessing are shown, and the answers revealed.
 *
 * The engine needs nothing new: hearing X just sends the hidden slot's
 * settings, exactly as switching to that slot would. While a test runs the
 * UI must not show which slot is playing; `analog.active` is therefore not
 * to be displayed then (the panel and the shortcut messages check `running`).
 */
export const useAbxStore = defineStore("abx", () => {
  const analog = useAnalogStore();

  const running = ref(false);
  const finished = ref(false);
  const total = ref(10);
  const heard = ref<Heard>("a");
  const trials = ref<Trial[]>([]);
  /** Number of answers given; the hidden truth of the current trial is not exposed. */
  const answered = computed(() => trials.value.length);

  let truth: Slot = "a"; // the current trial's hidden X (deliberately not reactive)
  let rng: () => number = Math.random;

  const levelDifference = computed(() => {
    const { a, b } = analog.measured;
    return a === null || b === null ? null : Math.abs(a - b);
  });
  const levelMatched = computed(() => levelDifference.value !== null && levelDifference.value <= MATCH_TOLERANCE_DB);
  /** Identical settings make the test meaningless. */
  const slotsDiffer = computed(() => JSON.stringify(analog.a) !== JSON.stringify(analog.b));

  function newTrial(): void {
    truth = rng() < 0.5 ? "a" : "b";
  }

  function play(h: Heard): void {
    heard.value = h;
    analog.select(h === "x" ? truth : h, true); // forced: the engine must get the slot even if it was already active
  }

  /** Begin a test of `count` trials. `random` is injectable for tests. */
  function start(count = 10, random: () => number = Math.random): void {
    rng = random;
    total.value = Math.max(1, Math.round(count));
    trials.value = [];
    finished.value = false;
    running.value = true;
    newTrial();
    play("a");
  }

  /** Listen to A, B or the hidden X. */
  function hear(h: Heard): void {
    if (!running.value) return;
    play(h);
  }

  /** Say which slot X is. Records the answer and moves on (or finishes). */
  function answer(guess: Slot): void {
    if (!running.value) return;
    trials.value = [...trials.value, { truth, guess }];
    if (trials.value.length >= total.value) {
      running.value = false;
      finished.value = true;
      play("a");
      return;
    }
    newTrial();
    play("x"); // stay on X for the next trial so the change is easy to hear
    heard.value = "x";
  }

  /** Abandon a test in progress. */
  function cancel(): void {
    running.value = false;
    finished.value = false;
    trials.value = [];
    heard.value = "a";
    analog.select("a", true);
  }

  /** Close the result. */
  function dismiss(): void {
    finished.value = false;
    trials.value = [];
  }

  const correct = computed(() => trials.value.filter((t) => t.truth === t.guess).length);
  const chance = computed(() => (finished.value ? guessingChance(trials.value.length, correct.value) : null));
  const verdict = computed(() => {
    if (!finished.value || chance.value === null) return null;
    const n = trials.value.length;
    const pct = (chance.value * 100).toFixed(chance.value < 0.001 ? 2 : 1);
    if (chance.value < 0.05) return `You got ${correct.value} of ${n} right. Guessing would do that only ${pct}% of the time: you can hear the difference.`;
    return `You got ${correct.value} of ${n} right. Guessing would do that ${pct}% of the time, so this test cannot show that you can hear the difference.`;
  });
  /** The answers, once the test is over. */
  const revealed = computed(() => (finished.value ? trials.value : []));

  return {
    running, finished, total, heard, answered, levelDifference, levelMatched, slotsDiffer,
    start, hear, answer, cancel, dismiss, correct, chance, verdict, revealed,
  };
});
