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
  /** BLAKE3 of the file's contents, as hex; null until the server has
   *  hashed it (a background job after the scan). */
  hash?: string | null;
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
  /** "White Album, The": sort by this; absent from older servers. */
  sort_title?: string | null;
  /** "Beatles, The". */
  sort_artist?: string | null;
  /** MusicBrainz release ID, from embedded tags (or later a lookup). */
  mbid?: string | null;
  /** Where the cover came from: "embedded" (or later "caa"). */
  artwork_source?: string | null;
}

export interface Artist {
  id: number;
  name: string;
  /** "Beatles, The": sort by this; absent from older servers. */
  sort_name?: string | null;
}

/** A canonical genre (`GET /api/genres`): raw genre tags map to these. */
export interface Genre {
  name: string;
  track_count: number;
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
export type BitPerfectMode = "auto" | "off" | "mqa" | "all";

/** Top-level sound-quality mode. Auto settings in Advanced follow it. */
export type QualityMode = "best" | "compatible";

export const BIT_PERFECT_MODES: BitPerfectMode[] = ["auto", "off", "mqa", "all"];

export interface PlayerState {
  status: PlayerStatus;
  track: Track | null;
  queue_ids: number[];
  queue_index: number | null;
  position_ms: number;
  duration_ms: number | null;
  /** How far the data received from the server reaches (ms); null when unknown. */
  buffered_ms?: number | null;
  /** Network speed of the stream in bytes/s; null until measured or when not read ahead. */
  download_bps?: number | null;
  /** Audio buffered beyond the playhead (ms); null when unknown. */
  buffer_ahead_ms?: number | null;
  /** The whole stream is already fetched (so a short buffer is just the track's end). */
  buffer_complete?: boolean;
  /** Playback ran out of buffered audio and is waiting for the network. */
  buffering?: boolean;
  /** Rate of the audio reaching the output (what the EQ is designed at); null when idle. */
  output_rate_hz?: number | null;
  /** What the analog stage is doing (plan, latency); null when off. */
  analog_plan?: string | null;
  /** How the analog stage changes the level; null when off or not yet measured. */
  analog_level?: AnalogLevel | null;
  /**
   * Look-ahead limiter gain reduction, dB (positive; 0 = not working). Null
   * when the limiter is off, or the path bypasses it (DoP / bit-perfect).
   */
  limiter_gr_db?: number | null;
  /**
   * DSP load EWMA: chain processing seconds per audio second. Above 1 the
   * chain is slower than real time. 0 when idle.
   */
  dsp_load?: number;
  /** Per-stage load EWMAs in chain order: [stage name, load]. */
  dsp_stage_load?: [string, number][];
  /** Audio-callback underruns since the stream opened. */
  underruns?: number;
  /** Playback speed (1 = as recorded); pitch is kept at any speed. */
  playback_rate?: number;
  /** The radio station playing: its song title and connection state; null otherwise. */
  radio?: RadioNow | null;
  format: string | null;
  chain: string | null;
  /** "pcm-shared" (DSP chain active) or "dop-exclusive" (bit-perfect). */
  output_path: OutputPathName;
  volume: number;
  error: string | null;
  /** Non-fatal note for this track, e.g. why DSD played as FLAC. */
  notice?: string | null;
  /** Your own processing (EQ, Loudness, Analog, Volume) that exclusive output would bypass right now. */
  exclusive_blockers?: string[];
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

/** The analog stage's effect on the level (K-weighted, smoothed over a few seconds). */
export interface AnalogLevel {
  input_lufs: number;
  output_lufs: number;
  /** Output minus input, dB: what the stage adds to the level. */
  delta_db: number;
  /** Output peak, dBFS, decaying about 6 dB per second. */
  peak_dbfs: number;
  /** Seconds of audio behind the reading. */
  seconds: number;
}

/** A ready-made A/B comparison: two slots to load, and what to play and listen for. */
export interface ListeningRecipe {
  id: string;
  title: string;
  /** What the comparison shows, in one sentence. */
  idea: string;
  /** What to play. */
  play: string;
  /** What to listen for. */
  listen: string;
  /** Settings for slot A and slot B (anything not given comes from the defaults and the flavour's typical values). */
  a: Partial<AnalogSettings>;
  b: Partial<AnalogSettings>;
}

const on = (flavour: AnalogFlavour, more: Partial<AnalogSettings> = {}): Partial<AnalogSettings> => ({
  enabled: true,
  flavour,
  ...more,
});
const dry: Partial<AnalogSettings> = { enabled: false };

/**
 * Suggested comparisons. Level-match by ear with the Output slider before you
 * judge: the louder side always sounds better.
 */
export const LISTENING_RECIPES: ListeningRecipe[] = [
  {
    id: "warmth-vs-dry",
    title: "Warmth against nothing",
    idea: "The plainest test: your music with and without a little 12AX7 colour.",
    play: "Acoustic guitar, piano or a solo voice, at a normal listening level.",
    listen: "A touch more body and a little sheen on the notes, without any change in loudness. If you cannot hear it, raise Drive to 60% and Mix to 60%.",
    a: dry,
    b: on("warm_triode", { drive: 0.5, mix: 0.5 }),
  },
  {
    id: "jfet-vs-300b",
    title: "Pure 2nd harmonic: JFET against 300B",
    idea: "Two kinds of even-harmonic warmth: the JFET's is nearly pure, the 300B adds transformer weight and sag.",
    play: "A male voice or a cello: something with a strong fundamental.",
    listen: "The JFET is smooth and clean-sounding with a fuller tone. The 300B is richer and heavier, with a slight softening on loud notes.",
    a: on("jfet", { drive: 0.6, mix: 0.7 }),
    b: on("tube_300b", { drive: 0.6, mix: 0.7 }),
  },
  {
    id: "el34-vs-6l6gc",
    title: "British against American power stages",
    idea: "Class-AB push-pull pairs: EL34s against 6L6GCs.",
    play: "Drums and electric guitar or bass, played fairly loud.",
    listen: "The EL34 pair is warmer, more compressed and a little gritty; the 6L6GC pair is cleaner and tighter, with a firmer low end.",
    a: on("push_pull_el34", { drive: 0.6, mix: 0.7 }),
    b: on("push_pull_6l6gc", { drive: 0.6, mix: 0.7 }),
  },
  {
    id: "class-a-vs-class-ab",
    title: "Class A against class AB at low level",
    idea: "A class-A pair of 2A3s is smooth at every level; a class-AB pair has crossover grit when quiet.",
    play: "A quiet, sparse passage: brushed drums, a solo instrument, or the tail of a fade-out.",
    listen: "The class-AB pair adds a slightly rough, buzzy edge to soft notes that the class-A pair does not.",
    a: on("push_pull", { drive: 0.6, mix: 1 }),
    b: on("push_pull_el34", { drive: 0.6, mix: 1 }),
  },
  {
    id: "drive-amount",
    title: "How much drive?",
    idea: "The same tube at two drive settings.",
    play: "A full mix with plenty of dynamics: rock, jazz combo or an orchestra.",
    listen: "At 30% the colour is a light glow. At 70% loud passages soften and thicken; watch for fatigue and use Output to keep the level even.",
    a: on("warm_triode", { drive: 0.3, mix: 0.5 }),
    b: on("warm_triode", { drive: 0.7, mix: 0.5 }),
  },
  {
    id: "small-signal-tubes",
    title: "Small-signal tubes: clean against colourful",
    idea: "A 12AU7 (low gain, clean) against a 12AX7 (high gain, more colour).",
    play: "Vocals and acoustic instruments with some room around them.",
    listen: "The 12AU7 is subtle and transparent; the 12AX7 is livelier, with more sheen on voices.",
    a: on("tube_12au7", { drive: 0.7, mix: 0.6 }),
    b: on("warm_triode", { drive: 0.7, mix: 0.6 }),
  },
  {
    id: "sag-and-iron",
    title: "Sag and transformer on drums and bass",
    idea: "No distortion curve at all: only the power supply sag and the transformer's bass saturation.",
    play: "Kick drum and bass guitar, or a synth bass, played loud.",
    listen: "With sag and transformer on, loud kicks give way slightly and then bloom back, and bass notes gain weight and grit. Off, they should sound exactly like the source.",
    a: on("iron_sag", { sag: 0, transformer: 0, mix: 1 }),
    b: on("iron_sag", { sag: 0.8, transformer: 0.8, mix: 1 }),
  },
  {
    id: "soft-vs-hard",
    title: "Soft clip against hard clip",
    idea: "Solid state that rounds off gently against one that clips abruptly.",
    play: "A loud, dense track. Cymbals and distorted guitars show it best.",
    listen: "The soft clip thickens and rounds; the hard clip turns harsh and buzzy as soon as it clips. This is the difference between pleasant and unpleasant overload.",
    a: on("solid_state", { drive: 0.8, mix: 0.8 }),
    b: on("hard_transistor", { drive: 0.8, mix: 0.8 }),
  },
  {
    id: "silicon-vs-germanium",
    title: "Symmetric against lopsided clipping",
    idea: "Silicon diodes clip both halves alike; a germanium diode against a silicon one clips one half sooner.",
    play: "An electric guitar or a synth lead, played loud.",
    listen: "The silicon pair sounds even and smooth; the germanium pair is rougher and more buzzy, with a fuzz-like edge.",
    a: on("silicon_diode", { drive: 0.7, mix: 0.8 }),
    b: on("germanium_diode", { drive: 0.7, mix: 0.8 }),
  },
  {
    id: "aliasing",
    title: "Does anti-aliasing matter?",
    idea: "Hard-driven distortion with no protection against the full 4x oversampling with ADAA.",
    play: "Bright material at 44.1 or 48 kHz: cymbals, hi-hats, or a high piano. Turn the volume down first.",
    listen: "Without protection, look for a fine, metallic fizz that is not part of the instrument. With it, the top end stays clean. (At 96 kHz and above the difference is much smaller.)",
    a: on("warm_triode", { drive: 1, mix: 1, antialias: "x1" }),
    b: on("warm_triode", { drive: 1, mix: 1, antialias: "x4_adaa" }),
  },
];

/** One line on a slot: "12AX7 · drive 50% · mix 50%", or "Off (dry signal)". */
export function describeAnalog(s: AnalogSettings): string {
  if (!s.enabled) return "Off (dry signal)";
  const pct = (v: number): number => Math.round(v * 100);
  return `${FLAVOUR_INFO[s.flavour].short} · drive ${pct(s.drive)}% · mix ${pct(s.mix)}%`;
}

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
  /** Gain applied with the EQ, dB; absent in settings files from before the preamp. */
  eq_preamp_db?: number;
  loudness_enabled: boolean;
  loudness_target: number;
  /** Absent in settings files from before the analog stage. */
  analog?: AnalogSettings;
  /** Absent in settings files from before the limiter; it defaults to off. */
  limiter_enabled?: boolean;
  /** Absent in settings files from before crossfeed; it defaults to off. */
  crossfeed?: CrossfeedSettings;
}

// --- Headphone crossfeed (mirrors kahawai-player-core/src/dsp/crossfeed.rs) ------

export type CrossfeedPreset = "bauer" | "chu_moy" | "meier" | "custom";

export interface CrossfeedSettings {
  enabled: boolean;
  preset: CrossfeedPreset;
  /** Low-pass cutoff of the crossfed path, Hz (used by Custom). */
  cutoff_hz: number;
  /** Level of the crossfed path, dB (used by Custom). */
  feed_db: number;
}

export const DEFAULT_CROSSFEED_SETTINGS: CrossfeedSettings = {
  enabled: false,
  preset: "bauer",
  cutoff_hz: 700,
  feed_db: 4.5,
};

/** Custom-mode ranges, as the engine clamps them. */
export const CROSSFEED_CUTOFF_RANGE = [200, 2000] as const;
export const CROSSFEED_FEED_RANGE = [0.5, 15] as const;

export const CROSSFEED_PRESETS: CrossfeedPreset[] = ["bauer", "chu_moy", "meier", "custom"];

export const CROSSFEED_PRESET_INFO: Record<
  CrossfeedPreset,
  { label: string; params: [cutoffHz: number, feedDb: number] | null; blurb: string }
> = {
  bauer: { label: "Bauer", params: [700, 4.5], blurb: "The default, via bs2b, and the strongest of the three: closest to listening to a pair of speakers." },
  chu_moy: { label: "Chu Moy", params: [700, 6.0], blurb: "A DIY-era favourite. A little less crossfeed than Bauer." },
  meier: { label: "Jan Meier", params: [650, 9.5], blurb: "From Jan Meier's Corda headphone amps. The mildest of the three." },
  custom: { label: "Custom", params: null, blurb: "Set the cutoff and feed yourself." },
};

/** The engine's clamp, so the UI shows what will actually be applied. */
export function clampCrossfeed(s: CrossfeedSettings): CrossfeedSettings {
  const num = (v: unknown, fallback: number) => (typeof v === "number" && Number.isFinite(v) ? v : fallback);
  const [cutLo, cutHi] = CROSSFEED_CUTOFF_RANGE;
  const [feedLo, feedHi] = CROSSFEED_FEED_RANGE;
  return {
    enabled: !!s.enabled,
    preset: CROSSFEED_PRESETS.includes(s.preset) ? s.preset : "bauer",
    cutoff_hz: Math.min(cutHi, Math.max(cutLo, num(s.cutoff_hz, 700))),
    feed_db: Math.min(feedHi, Math.max(feedLo, num(s.feed_db, 4.5))),
  };
}

export const DEFAULT_DSP_SETTINGS: DspSettings = {
  eq_bands: [],
  eq_enabled: true,
  eq_preamp_db: 0,
  loudness_enabled: false,
  loudness_target: -14,
  analog: DEFAULT_ANALOG_SETTINGS,
  limiter_enabled: false,
  crossfeed: DEFAULT_CROSSFEED_SETTINGS,
};

/** Room for an AutoEq profile (usually 10 filters) plus a couple of your own. Mirrors `MAX_EQ_BANDS` in dsp/eq.rs. */
export const MAX_EQ_BANDS = 12;

/** Range of the EQ preamp in dB. Mirrors `EQ_PREAMP_RANGE_DB` in dsp/eq.rs. */
export const EQ_PREAMP_RANGE_DB = [-24, 12] as const;

export interface OutputDevice {
  name: string;
  is_default: boolean;
}

export interface DsdRateSupport {
  name: "DSD64" | "DSD128" | "DSD256";
  /** PCM rate (Hz) the DoP stream for this DSD rate runs at. */
  dop_rate: number;
  supported: boolean;
}

/** What the selected output device reports it can carry. */
export interface DeviceCapabilities {
  name: string;
  /** "usb" | "thunderbolt" | "firewire" | "built-in" | "bluetooth" | "hdmi" | "airplay" | "virtual" | "other" | "unknown". */
  transport: string;
  /** External DAC-class connection: Best quality may take it exclusively. */
  external_dac: boolean;
  sample_rates: number[];
  /** Integer bit depths offered (a 32 usually carries 24 valid bits). */
  bit_depths: number[];
  float32: boolean;
  /** DoP PCM rates (Hz) it can carry: 176400 = DSD64, 352800 = DSD128, 705600 = DSD256. */
  dop_rates: number[];
  exclusive_available: boolean;
}

export interface DopStatus {
  /** DoP PCM rates (Hz) the output device accepts right now. */
  supported_rates: number[];
  /** The same, named by the DSD rate they carry. */
  dsd_rates?: DsdRateSupport[];
  /** True on macOS: the exclusive hog-mode path exists. */
  exclusive_available: boolean;
  /** The device output would use (system default resolved to its name). */
  device?: string | null;
  /** Built in, or confirmed by the user, as decoding DoP. */
  known_dsd_device?: boolean;
  /** The user's own confirmation is what makes it known. */
  user_confirmed?: boolean;
  /** What "Auto" DSD handling resolves to right now. */
  auto_resolves_to?: "native" | "convert";
  /** Everything the device reports it can carry. */
  capabilities?: DeviceCapabilities | null;
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
export type JobKind = "extract_iso" | "transcode" | "scan" | "hash_files" | "enrich_metadata" | "enrich_books" | "podcast_download";
/** `paused` and `cancelled` only happen to album info lookups (`enrich_metadata`). */
export type JobStatus = "queued" | "running" | "done" | "failed" | "paused" | "cancelled";

export interface JobInfo {
  id: string;
  kind: JobKind;
  label: string;
  payload?: string | null;
  /** 0..1 */
  progress: number;
  status: JobStatus;
  message?: string | null;
  /** Live file counts of a running scan (absent from older servers). */
  files?: { done: number; total?: number | null; per_sec?: number | null; mb_per_sec?: number | null; eta_at?: number | null } | null;
}

/** "1,234 files scanned" on a first scan, "1,234 of about 5,000 files, done around 10:42 PM"
 *  on a rescan; null before the server has reported any. */
export function jobFilesDetail(j: JobInfo): string | null {
  const f = j.files;
  if (!f) return null;
  const n = (v: number): string => Math.round(v).toLocaleString();
  if (f.total == null) return `${n(f.done)} files scanned`;
  const speed = f.mb_per_sec ? `, ${n(f.mb_per_sec)} MB/s` : "";
  const eta = f.eta_at ? `, done around ${new Date(f.eta_at).toLocaleTimeString(undefined, { timeStyle: "short" })}` : "";
  return `${n(f.done)} of about ${n(f.total)} files${speed}${eta}`;
}

/** A job is "active" while the client should keep polling for it. */
export function isJobActive(j: JobInfo): boolean {
  return j.status === "queued" || j.status === "running";
}

/** DSD handling preference (Settings → DSD). Persisted by the Rust core. */
export type DsdStory = "auto" | "native" | "convert";

export interface PlaybackPrefs {
  quality_mode?: QualityMode;
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

// --- Audiobooks (docs/v1/kahawai-audiobook-spec.md) ---------------------------

/** A book in the library list. A position is always `book_offset_ms`: ms from the start of the book. */
export interface Audiobook {
  id: number;
  root_id: number;
  title: string;
  author: string | null;
  narrator: string | null;
  series: string | null;
  series_index: number | null;
  year: number | null;
  cover_hash: string | null;
  duration_ms: number;
  added_at: number;
  finished_at: number | null;
  position_ms: number;
  last_played_at: number | null;
  /** 0 to 1. */
  progress: number;
  /** The book's folder. */
  path: string;
  /** The first file's format ("mp3", "m4a"…), bitrate in kbps, sample rate and channels. */
  format: string | null;
  bitrate: number | null;
  sample_rate: number | null;
  channels: number | null;
}

export interface AudiobookPart {
  id: number;
  track_id: number;
  part_index: number;
  title: string | null;
  start_offset_ms: number;
  duration_ms: number;
  /** The file's format, bitrate (kbps), sample rate and channels. */
  format?: string | null;
  bitrate?: number | null;
  sample_rate?: number | null;
  channels?: number | null;
}

export interface AudiobookChapter {
  id: number;
  part_id: number;
  title: string;
  start_offset_ms: number;
  duration_ms: number;
}

export interface AudiobookBookmark {
  id: number;
  book_id: number;
  book_offset_ms: number;
  name: string;
  note: string;
  created_at: number;
}

export interface AudiobookSettings {
  speed: number;
  skip_back_s: number;
  skip_forward_s: number;
}

export interface AudiobookDetail extends Audiobook {
  parts: AudiobookPart[];
  chapters: AudiobookChapter[];
  bookmarks: AudiobookBookmark[];
  settings: AudiobookSettings;
}

export interface AudiobookSession {
  id: number;
  started_at: number;
  ended_at: number;
  start_offset_ms: number;
  end_offset_ms: number;
  listened_ms: number;
}

// --- internet radio -------------------------------------------------------------

/** `player-state.radio`: what the playing station says, and whether the connection is up. */
export interface RadioNow {
  title: string | null;
  reconnecting: boolean;
  attempt: number;
  bitrate_kbps: number | null;
  reason: string | null;
}

/** A station from the online directory (`GET /api/radio/search`). */
export interface RadioStation {
  station_uuid: string;
  name: string;
  url: string;
  url_resolved: string | null;
  homepage: string | null;
  favicon: string | null;
  tags: string | null;
  country: string | null;
  language: string | null;
  codec: string | null;
  bitrate: number | null;
  clicks: number;
  /** HLS (.m3u8): not playable yet. */
  hls: boolean;
  needs_relay: boolean;
}

/** A saved station (`/api/radio/favorites`). */
export interface RadioFavorite {
  id: number;
  station_uuid: string | null;
  name: string;
  url: string;
  url_resolved: string | null;
  homepage: string | null;
  favicon: string | null;
  tags: string | null;
  country: string | null;
  language: string | null;
  bitrate: number | null;
  codec: string | null;
  manual: boolean;
  sort_order: number;
  added_at: number;
}

export interface RadioPlayInfo {
  name: string;
  url: string;
  codec: string | null;
  bitrate: number | null;
  needs_relay: boolean;
}

export interface RadioFacet {
  name: string;
  stations: number;
}

export interface RadioHeard {
  id: number;
  station_name: string;
  stream_title: string;
  played_at: number;
}

export type RadioOrder = "clickcount" | "votes" | "name" | "bitrate";

export interface RadioQuery {
  q?: string;
  tag?: string;
  country?: string;
  language?: string;
  order?: RadioOrder;
  limit?: number;
  offset?: number;
}

// --- podcasts -------------------------------------------------------------------

/** A subscription (`GET /api/podcasts/feeds`). */
export interface PodcastFeed {
  id: number;
  feed_url: string;
  title: string;
  author: string | null;
  description: string | null;
  link: string | null;
  image_url: string | null;
  language: string | null;
  explicit: boolean;
  last_fetched: number | null;
  /** Why the last refresh failed; null when it worked. */
  last_error: string | null;
  auto_download: boolean;
  keep_n: number;
  delete_played_after_days: number;
  sort_order: number;
  added_at: number;
  episode_count: number;
  unplayed_count: number;
  speed: number;
  skip_back_s: number;
  skip_forward_s: number;
  /** An episode that ends with nothing in Up Next goes on to the next unplayed one. */
  auto_advance: boolean;
}

export interface PodcastEpisode {
  id: number;
  feed_id: number;
  guid: string;
  title: string;
  description_html: string | null;
  published_at: number | null;
  duration_ms: number | null;
  enclosure_url: string;
  enclosure_type: string | null;
  enclosure_bytes: number | null;
  image_url: string | null;
  season: number | null;
  episode: number | null;
  link: string | null;
  downloaded: boolean;
  file_bytes: number | null;
  played_at: number | null;
  dropped_from_feed: boolean;
  position_ms: number;
  position_updated_at: number | null;
  feed_title: string;
  feed_image_url: string | null;
}

export interface PodcastEpisodeDetail extends PodcastEpisode {
  feed: PodcastFeed;
}

export interface PodcastFolder {
  path: string;
  custom: boolean;
  usable: boolean;
  episodes_downloaded: number;
  bytes_downloaded: number;
}

export type PodcastFeedSettings = Partial<
  Pick<PodcastFeed, "auto_download" | "keep_n" | "delete_played_after_days" | "speed" | "skip_back_s" | "skip_forward_s" | "auto_advance">
>;

export interface PodcastSubscribed {
  feed: PodcastFeed;
  episodes_added: number;
  warnings: string[];
}

export interface PodcastImported {
  added: number;
  already_subscribed: number;
  invalid: string[];
}
