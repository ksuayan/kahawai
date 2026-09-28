import { describe, expect, it } from "vitest";
import { dirChipText, dirStatus } from "./types";
import type { DirValidation } from "./types";

function validation(overrides: Partial<DirValidation> = {}): DirValidation {
  return {
    exists: true,
    is_dir: true,
    readable: true,
    writable: true,
    audio_files: 12,
    truncated: false,
    ...overrides,
  };
}

describe("dirChipText", () => {
  it("shows the plain count when the walk finished", () => {
    expect(dirChipText(validation({ audio_files: 12 }))).toBe("12 audio files");
    expect(dirChipText(validation({ audio_files: 1 }))).toBe("1 audio file");
  });

  // A real library is comma-worthy — six figures of files isn't unusual.
  it("formats large counts with thousands separators", () => {
    expect(dirChipText(validation({ audio_files: 125_000 }))).toBe("125,000 audio files");
  });

  // Regression: setup_validate_dir's quick preview is time-boxed (a folder
  // that large, or on a slow network share, can take longer than the
  // budget), and used to report the partial count as if it were the true
  // total — indistinguishable from a folder that really only has that many.
  it("marks a truncated count as a lower bound, not an exact total", () => {
    expect(dirChipText(validation({ audio_files: 125_000, truncated: true }))).toBe(
      "125,000+ audio files",
    );
  });

  it("still reports inaccessible/empty regardless of truncation", () => {
    expect(dirChipText(validation({ readable: false, truncated: true }))).toBe("not accessible");
    expect(dirChipText(validation({ audio_files: 0, truncated: true }))).toBe(
      "no audio files found",
    );
  });
});

describe("dirStatus", () => {
  it("is unaffected by truncation — ok/warn/err only depend on accessibility and count", () => {
    expect(dirStatus(validation({ truncated: true }))).toBe("ok");
    expect(dirStatus(validation({ audio_files: 0, truncated: true }))).toBe("warn");
  });
});
