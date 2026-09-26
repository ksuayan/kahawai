// Shared domain types. Shapes mirror the frozen server API (snake_case).
// The server is the source of truth at runtime; parsing stays defensive.

export type TrackFormat =
  | "mp3"
  | "flac"
  | "m4a"
  | "aac"
  | "wav"
  | "aiff"
  | "ogg_vorbis"
  | "opus"
  | "dsf"
  | "dff"
  | "sacd_iso"
  | "unknown";

export interface Track {
  id: number;
  path: string;
  hash: string;
  format: TrackFormat;
  sample_rate?: number | null;
  bit_depth?: number | null;
  channels?: number | null;
  duration_ms?: number | null;
  bitrate?: number | null;
  title?: string | null;
  album?: string | null;
  artist?: string | null;
  album_id?: number | null;
  track_no?: number | null;
  disc_no?: number | null;
  genre?: string | null;
  year?: number | null;
  missing: boolean;
  decodable: boolean;
  /** MQA-encoded FLAC (detected from its tags by the server). */
  mqa?: boolean;
  /** Sample rate of the master before MQA folding, when the file says. */
  original_sample_rate?: number | null;
}

export interface Album {
  id: number;
  title: string;
  artist?: string | null;
  year?: number | null;
  artwork_hash?: string | null;
  track_ids: number[];
  track_count: number;
}

export interface Artist {
  id: number;
  name: string;
}

export interface Playlist {
  id: number;
  name: string;
  track_ids: number[];
}

export interface Page<T> {
  items: T[];
  page: number;
  per_page: number;
  total: number;
}

export type PlayerStatus = "stopped" | "loading" | "playing" | "paused";

/** Which audio path the engine is using (drives badges and whether DSP/volume apply). */
export type OutputPathName = "pcm-shared" | "dop-exclusive" | "pcm-exclusive";

/** When to play through the exclusive, untouched bit-perfect path. */
export type BitPerfectMode = "off" | "mqa" | "all";

export const BIT_PERFECT_MODES: BitPerfectMode[] = ["off", "mqa", "all"];

export interface PlayerState {
  status: PlayerStatus;
  track: Track | null;
  queue_ids: number[];
  queue_index: number | null;
  position_ms: number;
  duration_ms: number | null;
  /** How far the data received from the server reaches (ms); null when unknown. */
  buffered_ms?: number | null;
  /** Rate of the audio reaching the output (what the EQ is designed at); null when idle. */
  output_rate_hz?: number | null;
  /** What the analog stage is doing (plan, latency); null when off. */
  analog_plan?: string | null;
  format: string | null;
  chain: string | null;
  /** "pcm-shared" (DSP chain active) or "dop-exclusive" (bit-perfect). */
  output_path: OutputPathName;
  volume: number;
  error: string | null;
  /** Queue repeat mode. */
  repeat: "off" | "all" | "one";
  /** Queue shuffle on/off. */
  shuffle: boolean;
}

/** Stream formats the core accepts for set_format / set_track_format. */
export type StreamFormat = "passthrough" | "flac" | "opus" | "mp3" | "dop";

export const STREAM_FORMATS: StreamFormat[] = ["passthrough", "flac", "opus", "mp3", "dop"];

/** Parametric EQ band type (wire shape is snake_case). */
export type EqBandType = "peaking" | "low_shelf" | "high_shelf" | "low_pass" | "high_pass";

export const EQ_BAND_TYPES: EqBandType[] = [
  "peaking",
  "low_shelf",
  "high_shelf",
  "low_pass",
  "high_pass",
];

/** One EQ band. `q` is RBJ Q for peaking/passes, shelf slope S for shelves. */
export interface EqBand {
  band_type: EqBandType;
  freq: number;
  gain_db: number;
  q: number;
}

/** UI row model: adds a per-row enable switch. Disabled rows are excluded
 *  from the band list sent to the core (the core has no per-band flag). */
export interface EqBandRow extends EqBand {
  enabled: boolean;
}

/** Analog warmth (tube / transistor character); mirrors `AnalogSettings` in the Rust core. */
export type AnalogFlavour =
  | "warm_triode"
  | "tube_12ax7a"
  | "tube_12at7"
  | "tube_12au7"
  | "tube_12ay7"
  | "tube_6sn7"
  | "tube_6sl7"
  | "tube_6dj8"
  | "tube_300b"
  | "tube_2a3"
  | "tube_el84"
  | "push_pull"
  | "push_pull_el34"
  | "push_pull_6l6gc"
  | "push_pull_kt88"
  | "solid_state"
  | "jfet"
  | "silicon_diode"
  | "germanium_diode"
  | "hard_transistor"
  | "iron_sag";
export type AntiAliasChoice = "auto" | "x1" | "x1_adaa" | "x2" | "x2_adaa" | "x4" | "x4_adaa";
export const ANALOG_FLAVOURS: AnalogFlavour[] = [
  "warm_triode",
  "tube_12ax7a",
  "tube_12at7",
  "tube_12au7",
  "tube_12ay7",
  "tube_6sn7",
  "tube_6sl7",
  "tube_6dj8",
  "tube_300b",
  "tube_2a3",
  "tube_el84",
  "push_pull",
  "push_pull_el34",
  "push_pull_6l6gc",
  "push_pull_kt88",
  "solid_state",
  "jfet",
  "silicon_diode",
  "germanium_diode",
  "hard_transistor",
  "iron_sag",
];

export interface FlavourInfo {
  /** Menu label. */
  label: string;
  /** Short name for summaries. */
  short: string;
  /** One line on what to expect. */
  blurb: string;
  /** Typical Sag and Transformer for this kind of stage (applied when it is chosen). */
  sag: number;
  transformer: number;
}

/** What each flavour is. The tube models are Koren's datasheet fits. */
export const FLAVOUR_INFO: Record<AnalogFlavour, FlavourInfo> = {
  warm_triode: {
    label: "12AX7 · high-mu preamp triode",
    short: "12AX7",
    blurb: "Soft, even-harmonic warmth from a high-gain preamp triode. The default.",
    sag: 0.3,
    transformer: 0.2,
  },
  tube_12at7: {
    label: "12AT7 (ECC81) · medium-high mu",
    short: "12AT7",
    blurb: "A little cleaner than the 12AX7, with a slightly firmer, more open sound.",
    sag: 0.15,
    transformer: 0,
  },
  tube_12au7: {
    label: "12AU7 (ECC82) · low mu, clean",
    short: "12AU7",
    blurb: "Low gain and low distortion: the mildest tube colour.",
    sag: 0.15,
    transformer: 0,
  },
  tube_6sn7: {
    label: "6SN7 · low-mu octal triode",
    short: "6SN7",
    blurb: "Smooth and full-bodied, a favourite line-stage tube.",
    sag: 0.15,
    transformer: 0,
  },
  tube_6dj8: {
    label: "6DJ8 (ECC88) · low-noise triode",
    short: "6DJ8",
    blurb: "Medium mu with a taut, detailed character.",
    sag: 0.15,
    transformer: 0,
  },
  tube_300b: {
    label: "300B · single-ended power triode",
    short: "300B",
    blurb: "The classic single-ended amplifier: rich 2nd harmonic, gentle overload, with transformer and sag.",
    sag: 0.4,
    transformer: 0.5,
  },
  tube_2a3: {
    label: "2A3 · single-ended power triode",
    short: "2A3",
    blurb: "Like the 300B, a little lighter and quicker.",
    sag: 0.4,
    transformer: 0.5,
  },
  push_pull: {
    label: "Push-pull tubes (2A3 pair)",
    short: "Push-pull",
    blurb: "Fuller and firmer: even harmonics cancel, odd ones and compression take over as it is driven.",
    sag: 0.5,
    transformer: 0.5,
  },
  tube_12ax7a: {
    label: "12AX7A (Sylvania) · high-mu preamp triode",
    short: "12AX7A",
    blurb: "A second 12AX7 fit with a slightly different curve: a touch more even harmonic at low levels.",
    sag: 0.3,
    transformer: 0.2,
  },
  tube_12ay7: {
    label: "12AY7 · low-noise, medium-mu triode",
    short: "12AY7",
    blurb: "Between the 12AU7 and the 12AT7: clean, with a gentle lift in the 2nd harmonic.",
    sag: 0.15,
    transformer: 0,
  },
  tube_6sl7: {
    label: "6SL7GT · high-mu octal triode",
    short: "6SL7",
    blurb: "Very clean until pushed, then it turns over abruptly: a big-headroom high-gain tube.",
    sag: 0.15,
    transformer: 0,
  },
  tube_el84: {
    label: "EL84 · single-ended pentode",
    short: "EL84",
    blurb: "Class A pentode: brighter and grittier than a triode, with both even and odd harmonics.",
    sag: 0.4,
    transformer: 0.5,
  },
  push_pull_el34: {
    label: "Push-pull EL34 · class AB",
    short: "EL34 pair",
    blurb: "British-style power stage: odd harmonics, firm compression, and a touch of crossover grit at low level.",
    sag: 0.5,
    transformer: 0.5,
  },
  push_pull_6l6gc: {
    label: "Push-pull 6L6GC · class AB",
    short: "6L6GC pair",
    blurb: "American-style power stage: cleaner and stiffer than EL34s.",
    sag: 0.3,
    transformer: 0.4,
  },
  push_pull_kt88: {
    label: "Push-pull KT88 · class AB",
    short: "KT88 pair",
    blurb: "A big, tight, high-power stage with plenty of headroom.",
    sag: 0.3,
    transformer: 0.5,
  },
  jfet: {
    label: "JFET · square-law warmth",
    short: "JFET",
    blurb: "Nearly pure 2nd harmonic with almost no 3rd, a very smooth kind of warmth.",
    sag: 0,
    transformer: 0,
  },
  silicon_diode: {
    label: "Silicon diode clipper · soft, symmetric",
    short: "Silicon diodes",
    blurb: "A logarithmic soft clip: odd harmonics that build gradually. Overdrive-pedal territory when pushed.",
    sag: 0,
    transformer: 0,
  },
  germanium_diode: {
    label: "Germanium diode clipper · asymmetric",
    short: "Germanium diodes",
    blurb: "One half clips earlier than the other: even and odd harmonics together, and a rougher edge.",
    sag: 0,
    transformer: 0,
  },
  iron_sag: {
    label: "Transformer and sag only · no distortion curve",
    short: "Iron and sag",
    blurb: "No tube or transistor curve: just the transformer's bass colour and the supply sag. Try it with high Sag and Transformer.",
    sag: 0.5,
    transformer: 0.7,
  },
  solid_state: {
    label: "Solid state · soft clip",
    short: "Solid state",
    blurb: "Symmetric and clean, with a soft odd-harmonic edge; like a transformer-coupled console preamp.",
    sag: 0,
    transformer: 0.3,
  },
  hard_transistor: {
    label: "Hard transistor · near-hard clip",
    short: "Hard transistor",
    blurb: "Clean until it clips, then harsh. For effect, not for fidelity.",
    sag: 0,
    transformer: 0,
  },
};
export const ANTI_ALIAS_CHOICES: AntiAliasChoice[] = ["auto", "x1", "x1_adaa", "x2", "x2_adaa", "x4", "x4_adaa"];

export interface AnalogSettings {
  enabled: boolean;
  flavour: AnalogFlavour;
  /** 0..1: how hard the signal is pushed into the curve. */
  drive: number;
  /** 0..1: parallel blend of the processed signal. */
  mix: number;
  /** Output trim, -6..6 dB. */
  output_db: number;
  /** Match the processed level to the dry level. */
  auto_gain: boolean;
  antialias: AntiAliasChoice;
  /** 0..1: power-supply sag (loud passages lower headroom and gain, then recover). */
  sag: number;
  /** 0..1: output-transformer colour (bass saturates as the level rises). */
  transformer: number;
}

export const DEFAULT_ANALOG_SETTINGS: AnalogSettings = {
  enabled: false,
  flavour: "warm_triode",
  drive: 0.4,
  mix: 0.4,
  output_db: 0,
  auto_gain: true,
  antialias: "auto",
  sag: 0.3,
  transformer: 0.3,
};

/** Pull every value into its allowed range (the core does the same). */
export function clampAnalog(s: AnalogSettings): AnalogSettings {
  const n = (v: number, d: number, lo: number, hi: number): number =>
    Math.min(hi, Math.max(lo, Number.isFinite(v) ? v : d));
  return {
    ...s,
    flavour: ANALOG_FLAVOURS.includes(s.flavour) ? s.flavour : "warm_triode",
    antialias: ANTI_ALIAS_CHOICES.includes(s.antialias) ? s.antialias : "auto",
    drive: n(s.drive, 0.4, 0, 1),
    mix: n(s.mix, 0.4, 0, 1),
    output_db: n(s.output_db, 0, -6, 6),
    sag: n(s.sag, 0.3, 0, 1),
    transformer: n(s.transformer, 0.3, 0, 1),
  };
}

export interface DspSettings {
  eq_bands: EqBand[];
  eq_enabled: boolean;
  loudness_enabled: boolean;
  loudness_target: number;
  /** Absent in settings files from before the analog stage. */
  analog?: AnalogSettings;
}

export const DEFAULT_DSP_SETTINGS: DspSettings = {
  eq_bands: [],
  eq_enabled: true,
  loudness_enabled: false,
  loudness_target: -14,
  analog: DEFAULT_ANALOG_SETTINGS,
};

export const MAX_EQ_BANDS = 8;

export interface OutputDevice {
  name: string;
  is_default: boolean;
}

export interface DopStatus {
  /** DoP PCM rates (Hz) the default output device accepts right now. */
  supported_rates: number[];
  /** True on macOS: the exclusive hog-mode path exists. */
  exclusive_available: boolean;
}

/**
 * Valid per-track format options shown in the now-playing picker.
 * - DSD (dsf/dff): transcode to FLAC, or native DoP to a DSD-capable DAC.
 * - unknown / sacd_iso (offline extraction): FLAC transcode only.
 * - Everything else is directly streamable: passthrough or transcode.
 */
export function validFormatsFor(t: Track): StreamFormat[] {
  if (t.format === "dsf" || t.format === "dff") return ["flac", "dop"];
  if (t.format === "unknown" || t.format === "sacd_iso") return ["flac"];
  return ["passthrough", "flac", "opus", "mp3"];
}

/** A track is playable only when its file is present and the core can decode it. */
export function isPlayable(t: Track): boolean {
  return t.decodable && !t.missing;
}

export function unplayableReason(t: Track): string {
  if (t.missing) return "File missing from disk";
  if (!t.decodable) return "Not decodable (needs offline extraction)";
  return "";
}

export function trackTitle(t: Track): string {
  return t.title?.trim() || t.path.split("/").pop() || `Track ${t.id}`;
}

export function formatBadge(t: Track): string {
  const f = t.format.toUpperCase().replace("_", " ");
  const parts: string[] = [f];
  if (t.bit_depth && t.sample_rate) {
    parts.push(`${t.bit_depth}/${Math.round(t.sample_rate / 100) / 10}k`);
  } else if (t.sample_rate) {
    parts.push(`${Math.round(t.sample_rate / 100) / 10}kHz`);
  }
  return parts.join(" · ");
}

/** Full detail behind the format badge: "24-bit / 96 kHz · 2647 kbps · 2 ch". */
export function qualityTitle(t: Track): string {
  const parts: string[] = [];
  if (t.bit_depth && t.sample_rate) {
    parts.push(`${t.bit_depth}-bit / ${Math.round(t.sample_rate / 100) / 10} kHz`);
  } else if (t.sample_rate) {
    parts.push(`${Math.round(t.sample_rate / 100) / 10} kHz`);
  }
  if (t.bitrate) parts.push(`${t.bitrate} kbps`);
  if (t.channels) parts.push(`${t.channels} ch`);
  const label = t.format.toUpperCase().replace("_", " ");
  return parts.length ? `${label} · ${parts.join(" · ")}` : label;
}

/** "MQA · 48k" (the master's rate) or just "MQA". */
export function mqaLabel(t: Track): string {
  const r = t.original_sample_rate;
  return r ? `MQA · ${Math.round(r / 100) / 10}k` : "MQA";
}

/** Tooltip that says what the badge means and what the player does with it. */
export function mqaTitle(t: Track): string {
  const rate = t.original_sample_rate ? ` (master ${Math.round(t.original_sample_rate / 100) / 10} kHz)` : "";
  return (
    `MQA-encoded${rate}. It plays as ordinary FLAC everywhere. To let an MQA-capable DAC ` +
    `decode it, turn on Bit-perfect output in Settings.`
  );
}

export function formatDuration(ms?: number | null): string {
  if (ms == null || !isFinite(ms) || ms < 0) return "--:--";
  const total = Math.floor(ms / 1000);
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  const mm = h > 0 ? String(m).padStart(2, "0") : String(m);
  return `${h > 0 ? h + ":" : ""}${mm}:${String(s).padStart(2, "0")}`;
}

// --- C3: jobs, DSD preference, playlist import -------------------------------

/** Server job (`GET /api/jobs`). Shapes mirror kahawai-core (snake_case). */
export type JobKind = "extract_iso" | "transcode" | "scan";
export type JobStatus = "queued" | "running" | "done" | "failed";

export interface JobInfo {
  id: string;
  kind: JobKind;
  label: string;
  payload?: string | null;
  /** 0..1 */
  progress: number;
  status: JobStatus;
  message?: string | null;
}

/** A job is "active" while the client should keep polling for it. */
export function isJobActive(j: JobInfo): boolean {
  return j.status === "queued" || j.status === "running";
}

/** DSD handling preference (Settings → DSD). Persisted by the Rust core. */
export type DsdStory = "native" | "convert";

export interface PlaybackPrefs {
  dsd_story: DsdStory;
  global_format: StreamFormat | null;
  bit_perfect?: BitPerfectMode;
}

/** Result of `POST /api/playlists/import`. */
export interface ImportPlaylistResult {
  playlist_id: number;
  matched: number;
  unmatched: string[];
}
