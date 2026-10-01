# Analog emulation: tube and transistor "euphonics"

Research, plan and results for adding tube and transistor character to the PCM playback chain. Branch: `analog-poc`.

**Status: built and tested; not yet listened to.** The engine has an optional analog stage (21 flavours, drive, mix, sag, transformer colour, level match, alias-protected), and Settings has an A/B test bench for it (two slots, level meter, blind test, listening suggestions, keyboard shortcut). It is off by default and never runs on DoP or bit-perfect output.

Written for the Kahawai maintainers. Related: [Backlog.md](../Backlog.md) (what is left), [EQ.md](EQ.md) (the EQ, and the `DspStage` seam this uses).

**Where to look**

| If you want to... | Read |
|---|---|
| Try it | 13 (the panel), 18 (listening suggestions) |
| Understand the terms and flavours | 17 |
| See what each flavour is made from | 15, 16 |
| See how it was built and measured | 10 to 14 |
| Check sources and how deeply they were read | 19 |
| See the original plan | 1 to 9 |

How to read the claims below:

- **[cited]** comes from a source listed in the References (section 19), found in this research pass. I read the search summaries, not the full papers, so treat details as leads to confirm in Phase 0.
- **[known]** is general audio-DSP background I am confident of but did not source here.
- **[guess]** is my estimate and must be measured before it is relied on.

---

## 1. Goal and non-goals

**Goal:** an optional, subtle "warmth" stage in the shared PCM path that can be switched among a few flavours (tube and transistor), with a drive control and a dry/wet mix, sounding good on music at normal listening levels and costing little CPU.

**Non-goals:**
- Not circuit-accurate emulation of a specific product. We model *character* (harmonics, level-dependent softness, a little dynamics), and name presets as flavours, not brands.
- Not a guitar-amp simulator (no tone stacks, cabinets, heavy clipping).
- Not on the exclusive paths: DoP and bit-perfect playback bypass it, as they do EQ, loudness and volume.

---

## 2. What makes tubes and transistors sound different (the target)

All **[known]** unless marked:

| Trait | Typical tube behaviour | Typical transistor / op-amp behaviour |
|---|---|---|
| Harmonic balance | A single-ended triode stage is asymmetric: mostly **2nd harmonic** (even), then 3rd, falling off smoothly | Symmetric clipping: mostly **3rd and other odd** harmonics; can be very clean below clipping |
| Onset | Distortion rises gradually with level ("soft knee") | Very low distortion until the rail, then an abrupt knee (hard clipping) |
| Dynamics | Power-supply sag and recovery compress loud passages slightly | Stiff supplies: little sag |
| Low end | Output transformer saturates at low frequencies and loud levels, rolls off the extremes | Direct coupled, or transformer in classic console preamps |
| Grid current | Positive grid drive draws current and clips hard on that side (**[cited]**: the Koren model includes a grid-cathode diode) | n/a |

At music levels the audible effect is a **low-order harmonic colouring that grows with level**, plus mild compression. Getting that right, without aliasing, matters more than circuit detail.

---

## 3. Research findings

### 3.1 Modelling approaches, from cheap to expensive

| Approach | What it is | Fit for us |
|---|---|---|
| **Static waveshaper** with asymmetry and filters around it | A memoryless curve (tanh, polynomial, or a table) placed between pre- and post-filters. The classic "tubes as filters + waveshapers" approach (**[cited]**: described as a common approach in the DAFx literature) | **Best first step.** Cheap, controllable, easy to test |
| **Fitted tube curve** (Koren) | Norman Koren's phenomenological triode equations fit datasheet curves well (**[cited]**). Koren's own 12AX7 set: mu 100, Ex 1.4, Kg1 1060, Kp 600, Kvb 300 (**[cited]**, read from his page in Phase 0; a different fit with mu 100.26, Ex 1.394, Kg1 1651.8, Kp 1000, Kvb 524 also circulates, so the set to use is a Phase 0 decision). Most physically informed tube simulations still use Koren's or Cardarilli's model (**[cited]**) | **Phase 2.** Use it to *derive* the static curve and its asymmetry, and to build a lookup table; do not solve the circuit per sample at first |
| **Wave digital filters (WDF)** | Simulates the whole stage (tube plus resistors and capacitors) with one nonlinear element solved without iteration. Well studied for triode stages (**[cited]**: Pakarinen and Karjalainen's enhanced WDF triode; the RT-WDF library) | A possible later upgrade (Phase 5). More faithful, more code and CPU |
| **State-space / SPICE-style solvers** | Newton iteration per sample | Too heavy and fragile for a background playback thread |
| **Neural black-box models** | Learn a device from recordings (**[cited]**: a 2018 paper on tube amplifier emulation) | Needs data and training tooling; hard to keep light and deterministic. Not for a first version |

### 3.2 Aliasing is the main quality risk

A nonlinearity creates harmonics above the original band. Without protection they fold back below Nyquist as inharmonic, harsh distortion. Two families of fixes:

- **Oversampling** (2x to 8x around the nonlinear part, with good anti-alias and decimation filters). Robust and simple to reason about. Costs CPU in proportion to the factor, and adds latency with linear-phase filters (**[known]**).
- **Antiderivative antialiasing (ADAA)**, introduced by Parker, Zavalishin and Le Bivic (DAFx 2016) and developed by Bilbao, Esqueda, Parker and Välimäki (**[cited]**). It reduces aliasing without oversampling, for memoryless nonlinearities. It needs the antiderivative of the curve (closed form for tanh and hard clip). There are extensions to stateful systems (**[cited]**). Often combined with a small oversampling factor.

Our sources are 44.1 to 192 kHz. Content at 96 kHz and above has a lot of headroom already, so the required oversampling depends on the source rate: **[guess]** 4x at 44.1/48 kHz, 2x at 88.2/96 kHz, none or ADAA-only above. Confirm by measuring aliasing (section 6).

### 3.3 Output transformer and power-supply sag

- Transformer saturation and hysteresis are usually modelled with the Jiles-Atherton theory (**[cited]**), which is accurate but implicit and hard to fit and run in real time (**[cited]**). Cheaper alternatives are a level-dependent low-shelf plus a soft clip, or a small hysteresis-like state (**[known]**). Start with the cheap version.
- Sag can be modelled as a slow (tens of milliseconds) envelope of the signal that lowers the stage's supply, which lowers headroom and gain (**[known]**). It is a control-rate effect and cheap.

### 3.4 Existing implementations (license matters)

- **libmksim** (Rust; WDF plus Newton solvers for tubes, diodes, transistors, op-amps; MIT). **Phase 0 verdict: reference at most, not a dependency.** The GitHub repository was created on 2026-03-08 and its four commits all landed within 11 minutes, it has 3 stars, is not on crates.io, and its README calls AVX-512 a placeholder. No track record.
- **RT-WDF** (C++ WDF library). **Verdict: skip.** No license file (GitHub shows none), last pushed in 2017, needs the Armadillo C++ library, and the material I could read does not show a triode example.
- **Antialiasing reference code:** Chowdhury's ADAA repository is BSD-3-Clause (usable as a reference); Parker's DAFX-AntiAliasing repository has no license (read, do not copy).
- Some well-known open-source emulations are GPL (**[known]**, not checked for any specific project). Rule from the Backlog: do not copy code from a project until its license is confirmed compatible.

---

## 4. Proposed design

### 4.1 Where it lives

- A new module, now [dsp/analog/](../../crates/kahawai-player-core/src/dsp/analog/) in kahawai-player-core (pure Rust, no platform imports, like the other DSP stages).
- A small trait so stages compose (this is the `DspStage` idea from EQ.md):

```rust
pub trait DspStage: Send {
    fn prepare(&mut self, sample_rate: u32, channels: u16);
    fn process(&mut self, interleaved: &mut [f32]);   // in place
    fn latency_frames(&self) -> u32 { 0 }
    fn reset(&mut self);
}
```

- Chain order in `pump_pcm` (see [engine.rs](../../crates/kahawai-player-core/src/engine.rs)): decode, resample, **EQ, analog stage**, loudness gain, volume, sink. The analog stage goes after the EQ so the user's EQ shapes what is driven, and before loudness and volume so drive is independent of listening level. (Open question Q3.)

```mermaid
flowchart LR
    in["input sample"] --> pre["input gain<br/>(Drive)"]
    pre --> hp["pre-filter<br/>(coupling high-pass)"]
    hp --> up["oversample<br/>(2x to 4x)"]
    up --> ns["nonlinear stage<br/>(asymmetric curve,<br/>+ sag envelope)"]
    ns --> dn["decimate"]
    dn --> xf["transformer colour<br/>(low shelf, soft clip)"]
    xf --> lp["post-filter<br/>(tone roll-off)"]
    lp --> gm["auto gain match"]
    gm --> mix["dry / wet mix"]
    mix --> out["output"]
    in --> mix
```

### 4.2 Controls

| Control | Meaning | Range |
|---|---|---|
| **Flavour** | Selects a preset set of the parameters below | see 4.3 |
| **Drive** | Input level into the nonlinearity | 0 to 100 percent |
| **Mix** | Parallel blend of the dry signal | 0 to 100 percent, default about 30 to 50 |
| **Output** | Level trim (auto gain-matched by default) | plus or minus a few dB |
| (advanced, later) Bias / asymmetry, Sag, Tone | Expose only if useful | |

Defaults must be conservative: a subtle change, never louder than dry.

### 4.3 Flavour presets (names describe character, not brands)

| Flavour | Character | Main ingredients |
|---|---|---|
| **Warm triode** | Soft, even-harmonic warmth | Asymmetric curve derived from a high-mu triode, gentle sag, light transformer colour |
| **Push-pull tube** | Fuller, slightly compressed | Symmetrical stage (odd bias with some even), more sag, more transformer colour |
| **Solid-state console** | Clean, punchy, a little edge at high drive | Symmetric soft curve with a slightly sharper knee, no sag, light transformer colour |
| **Hard transistor** | Aggressive, for effect | Symmetric hard-ish clip with ADAA |

Inspired-by references (for our own measurement targets only, not for the UI): single-ended triode line stages, push-pull power stages, classic transformer-coupled console preamps. Final list to be chosen in Phase 0.

---

## 5. Itemized plan

Sizes: **S** an hour or two, **M** a day, **L** several days. Each phase ends with something we can listen to and a green test suite.

### Phase 0: research and decisions (M) *(this document is the start)*

| # | Item | Output |
|---|---|---|
| 0.1 | Read the key sources in the References (section 19) in full (Koren tube models, ADAA, WDF triode, transformer emulation) | **Partly done.** Koren's equations and parameters read and used; ADAA basics implemented and measured. WDF and transformer papers not yet read in full |
| 0.2 | Choose reference behaviours: target harmonic profiles (2nd/3rd/5th versus drive level) for each flavour | **Provisional.** Profiles measured from the prototype (section 10.2); still to be checked against published measurements |
| 0.3 | Check libmksim and RT-WDF: maturity, maintenance, license, API fit | **Done.** Neither is a dependency (section 3.4) |
| 0.4 | Measure the CPU budget | **Done** for a prototype (section 10.4) |
| 0.5 | Decide the open questions in section 7 | **Waiting on you.** Recommendations are in section 10.5 |

### Phase 1: the seam and a working static stage (M to L) *(done, see section 11)*

| # | Item | Status |
|---|---|---|
| 1.1 | Extract the `DspStage` trait; make `ParametricEq` implement it | Done |
| 1.2 | `AnalogStage`: input gain, asymmetric waveshaper, dry/wet mix, auto gain match | Done (two flavours) |
| 1.3 | Oversampling wrapper, 2x and 4x, built once and reused | Done (Kaiser FIR, streaming) |
| 1.4 | Fade-in/out on parameter change and on/off | Done, including flavour changes |
| 1.5 | Wire into `pump_pcm` after the EQ; PCM shared path only; DoP and bit-perfect bypass | Done |
| 1.6 | Engine command, settings persistence | Done (the UI came later, section 13; the player state gained `analog_plan` and `analog_level`) |

### Phase 2: derive curves from tube models (M) *(done, see section 12)*

| # | Item | Status |
|---|---|---|
| 2.1 | Evaluate the Koren triode equations and produce a normalized transfer curve for a chosen tube and load line | Done, at runtime rather than as a stored table (built in about 20 ms on first use; tested against the model) |
| 2.2 | Replace the hand-made curve with a table lookup for the tube flavour | Done; harmonic profile matches the prototype targets (test) |
| 2.3 | Add ADAA for the table curve so the oversampling factor can drop | Done; measured. ADAA is nearly free and clearly helps, but the factor was kept because oversampling is cheap enough (section 12.3) |

### Phase 3: dynamics and transformer colour (M) *(done, see section 14)*

| # | Item | Status |
|---|---|---|
| 3.1 | Sag: slow envelope follower that lowers headroom and gain, with attack and recovery | Done |
| 3.2 | Transformer colour: cheap level-dependent low shelf plus soft clip; evaluate a hysteresis state as a follow-up | Done (bass soft clip); hysteresis not tried |
| 3.3 | Tone shaping: coupling and roll-off filters per flavour | Partly: the 10 Hz coupling high-pass exists; a per-flavour roll-off was judged not worth adding (section 14.3) |

### Phase 4: UI (M) *(done, differently from the plan: see sections 13 and 18)*

| # | Item | Status |
|---|---|---|
| 4.1 | "Warmth" button next to the EQ button, opening a modal in the EQ dialog's style | **Not done.** Built instead as a section of Settings, because it grew into an A/B test bench (two slots, level meter, blind test, listening suggestions). A quick-access button in the transport bar is still open (Backlog) |
| 4.2 | Flavour picker, Drive, Mix, Output; live preview with OK / Cancel | Done as live edits with an A/B switch (no OK / Cancel: the two slots are the safety net). Also Sag, Transformer, level match and the anti-aliasing plan |
| 4.3 | Dim with "not supported for this stream type" on exclusive paths | Done |
| 4.4 | Guardrails: limit drive, warn when the stage will clip the output; gain-matched A/B toggle | Done: ranges are enforced, the level meter warns about clipping, and Match B to A gain-matches |
| 4.5 | Tests: component, store, and a house-style check | Done |

### Phase 5: optional higher fidelity (L, only if Phases 1-4 are not enough)

| # | Item | Notes |
|---|---|---|
| 5.1 | WDF-based triode stage (or libmksim) for one flavour | Compare against the table version by ear and measurement |
| 5.2 | Jiles-Atherton transformer for one flavour | Only if the cheap version is clearly lacking |
| 5.3 | Move the EQ, analog and future stages into a small chain editor | Backlog: "Effects chain UI" |

---

## 6. Verification plan

Everything is deterministic DSP, so most of it is testable in Rust without listening. Listening tests still decide "does it sound good".

1. **Harmonic profile.** Feed a 1 kHz sine at several levels, take an FFT, and check the 2nd, 3rd and 5th harmonic levels versus drive against the flavour's targets (tolerance in dB).
2. **Aliasing.** Feed a high-frequency sine (for example 15 to 20 kHz at 44.1 kHz) hard, and check that no inharmonic component rises above a threshold below the fundamental. Run at 44.1, 96 and 192 kHz. Compare oversampling factors and ADAA.
3. **Transparency.** With the stage off, or mix 0, the output equals the input bit for bit. At low drive and low levels, distortion stays below a stated figure.
4. **No clicks.** Change parameters and toggle the stage while a tone plays; measure the largest sample-to-sample jump, as in the EQ tests.
5. **Level and gain match.** Loudness of the wet and dry signal within about 1 dB at default settings.
6. **Path rules.** DoP and bit-perfect output are bit-identical with the stage enabled (as the existing DoP test does for the EQ).
7. **Performance.** Cost per chunk at 44.1 and 192 kHz stereo stays under a set share of the chunk's real-time duration (budget from item 0.4).
8. **Listening.** A/B on a fixed set of tracks (acoustic, vocal, dense rock, electronic) with gain matching, before merging each phase.

---

## 7. Open questions (decide in Phase 0)

- **Q1. Which stages to offer first?** All four flavours in 4.3, or one tube and one transistor flavour?
- **Q2. Oversampling or ADAA first?** Oversampling is simpler and more robust; ADAA saves CPU. Plan: oversampling in Phase 1, ADAA in Phase 2.
- **Q3. Where in the chain?** After the EQ and before loudness and volume (proposed), or after loudness? It changes how drive relates to level.
- **Q4. Latency.** Linear-phase oversampling filters add a fixed delay (a few milliseconds). Accept it and compensate in the playhead, or use minimum-phase filters?
- **Q5. Gain match.** Automatic (safer for A/B) or manual?
- **Q6. libmksim.** Dependency, reference, or ignore?
- **Q7. Naming and legal.** Preset names describe character; confirm we are comfortable with "inspired by" notes in documentation only.

---

## 8. Risks

| Risk | Mitigation |
|---|---|
| CPU cost at 192 kHz stereo on the playback thread | Measure first (0.4); reduce oversampling at high rates; ADAA |
| Aliasing sounds harsh | Aliasing tests (6.2) gate every phase |
| "Warmth" is subjective and easy to overdo | Conservative defaults, gain-matched A/B, clip warning |
| Clipping past 0 dBFS (no limiter in the chain today) | Auto gain match, drive limits, and the headroom item in the Backlog |
| Licence contamination from borrowed code | Own implementations from papers; license check before reuse |
| Model accuracy claims | We claim character, not fidelity to a named product |

---

## 9. Sources

Moved to **References** at the bottom of this document (section 19).

---

## 10. Phase 0 findings

A throw-away prototype lives in [research/analog-spike/](../../research/analog-spike/) (a standalone Rust program, not part of the player; run it with `cargo run --release`, output saved in [RESULTS.txt](../../research/analog-spike/RESULTS.txt)). It is unoptimized research code, so treat the numbers as ballpark, measured on an Intel Core i9-9900K.

### 10.1 What the prototype has

- Four memoryless shapers: an asymmetric tanh (a bias term adds even harmonics), a symmetric tanh, a hard clip, and a **12AX7 triode stage computed from Koren's equations** (300 V supply, 100 kΩ load, about −1.5 V bias) turned into a lookup table.
- Anti-aliasing options: none, first-order ADAA (analytic antiderivative), and 2x, 4x and 8x oversampling (Kaiser-windowed FIR, 32 taps per phase).
- The existing 8-band EQ from the player, for a CPU baseline.

### 10.2 Harmonic profiles (dB relative to the fundamental)

| Shaper | Input level | 2nd | 3rd | 4th | 5th | THD |
|---|---|---|---|---|---|---|
| 12AX7 stage (Koren) | 0.1 | −42 | −79 | −128 | −153 | 0.8% |
| | 0.3 | −33 | −60 | −99 | −115 | 2.4% |
| | 0.6 | −26 | −48 | −82 | −91 | 4.8% |
| | 1.0 | −22 | −38 | −71 | −76 | 8.4% |
| Asymmetric tanh (bias 0.4, gain 1.5) | 0.1 | −31 | −60 | −82 | −130 | 2.8% |
| | 0.3 | −22 | −40 | −54 | −88 | 8.0% |
| | 1.0 | −17 | −20 | −29 | −41 | 18% |
| Symmetric tanh (gain 2) | 0.1 | none | −50 | none | −98 | 0.3% |
| | 0.3 | none | −31 | none | −61 | 2.8% |
| | 1.0 | none | −16 | none | −28 | 17% |
| Hard clip (gain 2) | 0.1 to 0.3 | none | none | none | none | 0% |
| | 0.6 | none | −24 | none | −30 | 7.3% |
| | 1.0 | none | −13 | none | −27 | 23% |

What this shows:

- The **triode stage behaves like the textbook**: the 2nd harmonic dominates (about 20 dB above the 3rd), and it rises 1 dB per dB of input, while the 3rd rises 2 dB per dB. That is the "warm" signature and it falls out of Koren's equations with no tuning.
- A **symmetric curve gives only odd harmonics** (no 2nd or 4th), as expected for push-pull and transistor stages.
- A **hard clip is perfectly clean until it clips**, then produces strong odd harmonics. That is the "hard transistor" flavour, and it is why a soft knee is much more pleasant at moderate levels.
- The asymmetric tanh is a workable, cheaper stand-in for the triode, but its harmonics rise faster than the real curve's. Its 4th harmonic is much higher than the triode's.

These are **provisional targets** (item 0.2): they come from my model, not from published measurements of real tubes.

### 10.3 Aliasing (the risk that decides the design)

Level of everything in the audible band that is *not* a harmonic of the tone (that is aliasing), relative to the tone. Heavy test: asymmetric tanh at gain 3 and input 0.8, deliberately harsh.

| Sample rate, tone | No protection | ADAA (1st order) | 2x oversampling | 4x | 8x |
|---|---|---|---|---|---|
| 44.1 kHz, 6 kHz | −26 dB | −38 dB | −63 dB | −106 dB | −105 dB |
| 44.1 kHz, 9.5 kHz | −14 dB | −26 dB | −42 dB | −91 dB | −104 dB |
| 96 kHz, 9.5 kHz | −51 dB | −75 dB | −101 dB | −133 dB | −130 dB |

- At 44.1 kHz, unprotected aliasing is **very audible** (as loud as −14 to −26 dB relative to the tone). It has to be handled.
- **4x oversampling is clean** at 44.1 and 48 kHz (better than −90 dB). 2x is adequate for gentle drive but not for a hard test tone.
- **First-order ADAA alone is not enough at 44.1 kHz** for heavy drive (−26 to −38 dB), but it is good at 96 kHz and above (−75 dB). ADAA combined with 2x oversampling is the standard compromise.
- At 96 kHz and above, **2x is plenty**, and unprotected is already −51 dB for this harsh case.

A correction to my own first run: an early version of this test reported about −29 dB for every oversampling factor. That was a bug in the test (the filter's end-of-buffer edge and passband droop were being counted as aliasing), not a property of oversampling. The numbers above are after the fix.

### 10.4 CPU (one second of stereo audio, one core, this machine)

| Sample rate | Existing 8-band EQ | Naive shaper | Triode table | ADAA1 | 2x OS | 4x OS |
|---|---|---|---|---|---|---|
| 44.1 kHz | 0.2% | 0.1% | 0.1% | 0.2% | 1.1% | 2.1% |
| 96 kHz | 0.4% | 0.2% | 0.1% | 0.5% | 2.3% | 4.8% |
| 192 kHz | 0.8% | 0.5% | 0.2% | 1.1% | 4.7% | 9.8% |

- The shaper itself is nearly free; **oversampling is the cost**, and even a crude implementation of it (a direct FIR in f64) fits comfortably.
- **CPU is not the constraint** for this feature. A 4x oversampled stage at 192 kHz is about 10% of a core in unoptimized code. A polyphase half-band or f32 implementation should be several times cheaper (**[guess]**).
- Practical rule that follows: **4x at 44.1 and 48 kHz, 2x at 88.2 and 96 kHz, ADAA-only or none at 176.4 kHz and above.**

### 10.5 Options and my recommendations

| Question | Options | Recommendation |
|---|---|---|
| **Q1. Which flavours first?** | All four; or two | Start with **two: warm triode (table from Koren) and solid-state (symmetric soft curve)**. They are the most different, both are cheap, and they exercise the whole design. Add push-pull and hard transistor after |
| **Q2. Oversampling or ADAA?** | ADAA only; oversampling only; both | **Oversampling first** (robust, easy to test), with the factor chosen by sample rate as above. Add ADAA later only to lower the factor, or on the hard-clip flavour where oversampling alone is weaker |
| **Q3. Where in the chain?** | After EQ, before loudness and volume; or after loudness | **After EQ, before loudness and volume**, so drive does not depend on how loud you play |
| **Q4. Latency** | Linear-phase filters; minimum-phase filters | Linear phase (it is what the prototype measured). The delay is a few milliseconds at 44.1 kHz, so compensate it in the playhead, or accept it. Revisit only if it shows up |
| **Q5. Gain match** | Automatic; manual | **Automatic gain match by default** so an A/B is fair, with an Output trim |
| **Q6. libmksim / RT-WDF** | Depend; reference; ignore | **Ignore as dependencies.** Read libmksim for ideas if useful. Build our own table-based triode and a simple stage |
| **Triode model source** | Koren's set (mu 100, Ex 1.4, Kg1 1060, Kp 600, Kvb 300); the other circulating fit | Use Koren's page as the primary source and check the curve against the RCA/Philips datasheet plate curves before trusting it |
| **Higher fidelity later** | WDF for one flavour; neural model | Only if listening tests say the table version is not enough |

### 10.6 What is still open after Phase 0

- Published harmonic measurements to replace the provisional targets (10.2).
- Whether the table-based triode sounds right by ear; the prototype only measures it.
- Transformer and sag models (Phase 3) were researched but not prototyped.
- A quick check of the prototype's triode curve against a datasheet (the Koren model gave about 0.94 mA at a grid bias of −2 V and 250 V plate for a 12AX7, which is plausible but not verified).

---

## 11. Phase 1 results

Code: [dsp/analog/stage.rs](../../crates/kahawai-player-core/src/dsp/analog/stage.rs) (the stage), the `DspStage` trait in [dsp/mod.rs](../../crates/kahawai-player-core/src/dsp/mod.rs), and the wiring in [engine.rs](../../crates/kahawai-player-core/src/engine.rs). Tauri command: `set_analog`. (The Settings panel came later; see section 13.)

### 11.1 What exists

- **`DspStage` trait** (`prepare(sample_rate)`, `process(interleaved, channels)`, `latency_frames()`, `reset()`), implemented by both the EQ and the analog stage. The plan's sketch had `prepare(rate, channels)` and a `process` without a channel count; the built version follows the EQ's existing `process(samples, channels)` shape so nothing else had to change.
- **`AnalogStage`**, with these settings (all clamped, saved in `engine-settings.json` under `dsp.analog`, off by default; older settings files load fine):

| Setting | Meaning | Range / default |
|---|---|---|
| `enabled` | Stage on or off | off |
| `flavour` | `warm_triode` (asymmetric, even harmonics) or `solid_state` (symmetric, odd harmonics) | warm_triode |
| `drive` | How hard the signal is pushed into the curve | 0 to 1, default 0.4 |
| `mix` | Parallel blend of the processed signal | 0 to 1, default 0.4 |
| `output_db` | Output trim | plus or minus 6 dB, default 0 |
| `auto_gain` | Match the processed level to the dry level (at a −12 dBFS RMS reference sine) | on |

- **Oversampling by sample rate:** 4x up to 50 kHz, 2x up to 100 kHz, none above (`oversample_factor`), as recommended in section 10.5.
- **Latency:** while oversampling is active (up to 100 kHz) the FIR delays the signal 32 frames: 0.73 ms at 44.1 kHz, 0.67 ms at 48 kHz, 0.33 ms at 96 kHz; none above 100 kHz. The dry path is delayed by the same amount so the mix is time-aligned. The delay is **not** yet compensated in the playhead (under a millisecond); `latency_frames()` reports it for later.
- **Live changes:** drive, mix, output and gain match glide over about 15 ms; on/off cross-fades; a flavour change fades out, swaps the curve, and fades back in, so nothing clicks. Fully off, the stage is bit-transparent.
- **Bypass rules:** it runs only on the shared PCM path, after the EQ and before loudness and volume. DoP and bit-perfect never call it (the two existing bit-identical tests now switch the stage on, hostile settings included, and still pass).

### 11.2 How the curves behave

- **Warm triode** is an asymmetric tanh, `tanh(g x + bias) − tanh(bias)`, scaled to unity small-signal gain, with the bias growing with the square root of drive. At zero drive it is linear and clean; even a little drive is lopsided. A DC blocker (10 Hz) removes the offset the asymmetry creates. At moderate drive the 2nd harmonic sits about 24 dB above the 3rd (drive 0.4, input level 0.2: 2nd −21 dB, 3rd −45 dB relative to the tone).
- **Solid state** is a symmetric `tanh(g x) / g`: no even harmonics (better than −70 dB in the test), a soft odd-harmonic edge that grows with level.
- **Warm triode in Phase 1 was an approximation** (an asymmetric tanh whose bias scales with drive, because a fixed bias lost the even-harmonic dominance when driven). **Phase 2 replaced it with the curve derived from the Koren tube model** (section 12); the description above is kept as history. The solid-state curve is unchanged.

### 11.3 Tests (all pass)

The stage's tests are in `dsp/analog/tests.rs` (and each file's own unit tests), plus two engine tests:

- bit-transparent when off, and again after fading out;
- warm triode: 2nd harmonic well above the 3rd; solid state: no 2nd, audible 3rd;
- harmonics grow with level and drive, and a quiet signal at zero drive stays clean;
- aliasing stays below the audible threshold: better than −60 dB at 44.1 and 48 kHz and better than −75 dB at 96 kHz, on the hard test tone that aliased at −14 dB without protection;
- dry and wet are time-aligned (a linear-regime tone comes out as the input delayed by exactly the reported latency);
- auto gain keeps the processed level within 1 dB of the dry level, for both flavours at low and full drive;
- live parameter changes and flavour changes do not click (the tests fail if the fade is removed; I checked by shortening it);
- settings are clamped, and missing fields take defaults;
- engine: the stage colours PCM output, is off by default, and persists; DoP and bit-perfect bytes are identical with it on.

### 11.4 CPU (real stage, release build, one second of stereo audio)

| Sample rate | Existing 8-band EQ | Warm triode | Solid state |
|---|---|---|---|
| 44.1 kHz (4x) | 0.2% | 3.4% | 3.3% |
| 96 kHz (2x) | 0.4% | 3.8% | 3.8% |
| 192 kHz (none) | 0.9% | 1.0% | 0.9% |

Measured by [phase1_cost.rs](../../research/analog-spike/src/bin/phase1_cost.rs) (output in [PHASE1_COST.txt](../../research/analog-spike/PHASE1_COST.txt)). Plenty of room. The FIR is a straightforward implementation; there is obvious headroom for optimisation if it is ever needed.

### 11.5 How to try it (the Settings panel in section 13 replaces this)

Before the Settings panel existed, the stage could only be switched on by editing the file:

Quit the app, edit `engine-settings.json` in the app's data folder (`~/Library/Application Support/com.suayan.kahawai-player/` on macOS), and add this inside the `"dsp"` object:

```json
"analog": { "enabled": true, "flavour": "warm_triode", "drive": 0.5, "mix": 0.5,
            "output_db": 0.0, "auto_gain": true }
```

Start the app and play something on the normal (shared) output. Use `"solid_state"` for the other flavour. It will not apply on DoP or bit-perfect output.

### 11.6 Known limits and what is next

- (Since added: the A/B panel in Settings, section 13.)
- (Phase 2 since added the Koren-derived curve and ADAA.) Still no sag or transformer colour (Phase 3).
- No clip protection: with the output trim up or hot material and high drive, the result can exceed 0 dBFS. The gain-match reference is a −12 dBFS RMS sine, so loud music will not be perfectly level-matched.
- Switching the stage on from bypass shifts the un-delayed input to a 32-frame delayed path during the 15 ms fade; that is inaudible in practice but is a small comb-filter moment.

---

## 12. Phase 2 results

Code: the tube model and table in [dsp/analog/models.rs](../../crates/kahawai-player-core/src/dsp/analog/models.rs), ADAA in [dsp/analog/shaper.rs](../../crates/kahawai-player-core/src/dsp/analog/shaper.rs). Measurements come from `cargo test --release -p kahawai-player-core --lib measure_anti_alias_plans -- --ignored --nocapture` (aliasing) and [phase1_cost.rs](../../research/analog-spike/src/bin/phase1_cost.rs) (CPU, output in [PHASE1_COST.txt](../../research/analog-spike/PHASE1_COST.txt)).

### 12.1 The triode curve

- **Model:** Koren's triode equations with his 12AX7 parameters (mu 100, Ex 1.4, Kg1 1060, Kp 600, Kvb 300), in a stage with 300 V supply, 100 kΩ plate load and a fixed −1.5 V bias. The plate voltage is solved on the load line for each grid voltage.
- **Grid conduction:** when the grid swings positive it conducts (about 2 kΩ against a 10 kΩ source), which squashes that half of the wave. This is a smooth (0.05 V knee) approximation, not a circuit simulation. It is what makes the positive side clip and the negative side (cut-off) roll off, so the two halves compress differently.
- **Curve:** inverted so polarity is kept, zero at zero, unit slope at zero (so a quiet signal passes unchanged), tabulated at 4096 points over ±4 with linear interpolation. Input scale: 1.5 V of grid swing per unit.
- **Antiderivative:** the integral of the interpolated curve is computed exactly (piecewise quadratic), so first-order ADAA is exact for the table. The solid-state curve uses tanh with the analytic antiderivative `ln cosh`.
- **Cost of building it:** 20 ms on first use (release build), once per process, on the first enable. Not stored; if that ever matters, generate it at build time.
- **Zero drive** is no longer perfectly linear, because the tube is inherently lopsided (2nd harmonic about −48 dB at an input of 0.05). It is very clean, not bit-clean; only the stage being off is bit-transparent.

### 12.2 Harmonic profile (2.2)

At zero drive the stage reproduces the prototype from section 10.2: 2nd harmonic between −35 and −30 dB at an input of 0.3 (prototype −32.5), the 3rd below −52 dB (prototype −60), the 2nd rising about 1 dB per dB of input, and the even harmonics still dominating the odd ones at full swing. These are tested. They match because the curve is the same model; they are still **provisional targets** (no published measurements yet, section 10.6).

### 12.3 Aliasing and cost: ADAA versus oversampling (2.3)

Level of everything audible that is not a harmonic of the tone, relative to the tone. Hard test tone: drive 0.53, input 0.8, 9.5 kHz (effective curve input 2.4, deep into clipping). Lower is better.

| Curve, rate | 1x | 1x + ADAA | 2x | 2x + ADAA | 4x | 4x + ADAA |
|---|---|---|---|---|---|---|
| Triode, 44.1 kHz | −14 | −25 | −35 | −53 | −49 | **−71** |
| Triode, 96 kHz | −38 | −56 | −50 | **−74** | −66 | −95 |
| Solid state, 44.1 kHz | −14 | −24 | −47 | −70 | −90 | −106 |
| Solid state, 96 kHz | −47 | −71 | −101 | −123 | −122 | −121 |

At a typical setting (drive 0.4, input 0.3, 5 kHz) every combination is already below −89 dB for the triode (1x without ADAA at 44.1 kHz gives −89; everything else is at or below −96), so the differences above only appear when the curve is driven very hard.

What it shows:

- **ADAA helps a lot for almost no CPU** (about 0.1 percentage point for the triode, a bit more for tanh, which needs an exp and a log).
- **2x + ADAA beats 4x without ADAA** at 44.1 kHz for the triode (−53 versus −49) at half the cost, and it is far better than 2x alone.
- **The triode curve aliases more than tanh**, because its cut-off and grid conduction corners are sharper. That is why it needs more protection than the solid-state curve.

CPU per second of stereo audio, one core (release build):

| | 1x | 1x + ADAA | 2x | 2x + ADAA | 4x | 4x + ADAA |
|---|---|---|---|---|---|---|
| Triode, 44.1 kHz | 0.2% | 0.2% | 1.7% | 1.7% | 3.3% | 3.4% |
| Triode, 96 kHz | 0.4% | 0.5% | 3.6% | 4.0% | 7.0% | 7.3% |
| Triode, 192 kHz | 0.8% | 1.0% | 7.9% | 7.9% | 14.3% | 14.8% |
| Solid state, 44.1 kHz | 0.2% | 0.3% | 1.9% | 2.1% | 3.5% | 4.0% |
| Solid state, 96 kHz | 0.4% | 0.6% | 4.0% | 4.5% | 8.5% | 10.1% |

**Decision:** ADAA is now on at every rate. The oversampling factor is **not** reduced, because at these costs the extra quality is cheap: **4x + ADAA up to 50 kHz** (triode −71 dB on the hard tone), **2x + ADAA up to 100 kHz** (−74 dB), **1x + ADAA above** (about −56 dB at 96 kHz for 1x, and better still at 192 kHz; measured to −70 dB or better in the test). If CPU ever becomes a concern, dropping to 2x + ADAA at 44.1 kHz (−53 dB on the hard tone, 1.7% of a core) or 1x + ADAA at 96 kHz (−56 dB, 0.5%) is the fallback. The plan lives in `anti_alias_plan`.

### 12.4 What changed in behaviour

- The warm-triode flavour sounds different from Phase 1: the curve now comes from the tube model, with a real cut-off and grid-conduction corner.
- ADAA adds half a sample of delay to the wet path (about 2.6 µs at 192 kHz); not compensated.
- Tests added or changed (all pass): Koren current sanity, the table (unit slope, monotonic, asymmetric, agrees with the model), antiderivative consistency, ADAA equivalence for slow signals and constants, the harmonic profile, and tighter aliasing thresholds (better than −65 dB at 44.1 and 48 kHz, and −70 dB at 96 and 192 kHz).

### 12.5 Still open

- Compare the triode curve with datasheet plate curves and tune the swing, bias and load (the current current-level check is loose: 0.7 to 1.6 mA against a datasheet-scale 1.2 mA).
- Whether it sounds right by ear; only measured so far.
- Push-pull and hard-transistor flavours (Phase 3 and later), sag and transformer colour (Phase 3), UI (Phase 4): all since done, sections 13 to 16.

---

## 13. A/B test panel (Settings)

Settings, section **Analog warmth (experimental)**, right after the Parametric EQ. Code: [AnalogSection.vue](../../player/ui/src/components/AnalogSection.vue), [stores/analog.ts](../../player/ui/src/stores/analog.ts).

### 13.1 How it works

- **Two slots, A and B**, each a complete set of settings: *Warmth on*, *Flavour* (warm triode or solid state), *Drive*, *Mix*, *Sag*, *Transformer*, *Output* trim, *Match level to the dry signal*, and *Anti-aliasing*.
- **Listening to A / B** chooses which slot the engine plays. **Switch A/B** toggles. The switch fades out and in (about 15 ms per step), so it never clicks, and it works while music plays.
- **A starts as "off"** (the dry signal) and **B as the warm triode**, so the first comparison is warmth against nothing. Change either slot to compare anything else: triode against solid state, drive 30% against 70%, 4x + ADAA against 1x with no protection, and so on.
- **Edits to the slot you are hearing apply live.** Edits to the other slot wait until you switch to it.
- **Copy A to B** (and B to A) duplicates a slot, which is the quickest way to change one setting and compare.
- The panel says what the engine is doing right now, taken from the player state: for example "Now playing with 4x oversampling + ADAA, 0.7 ms latency." It says the stage is off when it is off or nothing plays on the shared output.
- On DoP and bit-perfect output the panel is dimmed with "Analog warmth is not supported for this stream type." (the same rule as the EQ).

### 13.2 Anti-aliasing choices (for listening tests)

*Auto* follows the sample rate (section 12.3). The others force a plan, so you can hear what the measurements in section 12 mean: **1x, no protection**; **1x + ADAA**; **2x**; **2x + ADAA**; **4x**; **4x + ADAA**. A plan change is faded like any other (out, swap, in). The engine also reports the latency each plan adds (0 ms for 1x; about 0.7 ms at 44.1 kHz with oversampling).

### 13.3 Level matching

"Match level" scales the processed signal so a −12 dBFS RMS sine comes out at the dry level. Real music is louder and denser, so it is **not** a perfect match; when you compare, use **Output** (plus or minus 6 dB) to even out what you hear, otherwise the louder side will sound better.

### 13.4 Where things are stored

- The engine saves the slot being heard in `engine-settings.json` (as before), so playback keeps working with no UI.
- The pair (both slots and which one is active) is kept by the app in its browser storage. Clearing app data resets it; the engine setting stays.

### 13.5 What was added underneath

- The core's settings gained `antialias` (`auto`, `x1`, `x1_adaa`, `x2`, `x2_adaa`, `x4`, `x4_adaa`); a changed plan or flavour waits for the fade-out before swapping.
- The player state gained `analog_plan` (text such as "4x oversampling + ADAA, 0.7 ms latency").
- Tests: plan mapping and serialization, a forced plan surviving a sample-rate change, live plan changes without clicks, the state field, the store (slot handling, persistence, clamping) and the component (A/B switching, live edits, all choices, dimming on exclusive output).

### 13.6 Keyboard shortcut

From any screen (not while typing in a text box, and not with Ctrl, Cmd or Alt held):

| Key | Does |
|---|---|
| **A** | Listen to slot A |
| **B** | Listen to slot B |
| **X** | Switch between A and B |

A small message ("Analog warmth: listening to B", with a one-line summary of the slot) appears for a couple of seconds so you can tell what you are hearing even when the Settings page is not on screen; each press replaces the last message. The keys are listed in Settings, under Keyboard shortcuts, and the A/B buttons show them in their tooltips.

### 13.7 Level meter and level matching

A/B comparisons are only fair when both sides are equally loud, so the panel now measures it. Code: `LoudnessMeter` in [dsp/loudness.rs](../../crates/kahawai-player-core/src/dsp/loudness.rs), the wiring in [engine.rs](../../crates/kahawai-player-core/src/engine.rs), the panel in [AnalogSection.vue](../../player/ui/src/components/AnalogSection.vue) and the matching logic in [stores/analog.ts](../../player/ui/src/stores/analog.ts).

**What it measures.** The engine measures the loudness of the signal going into the stage and coming out of it, and reports the difference: *what the stage adds to the level*. The measurement:

- uses the same K-weighting as the loudness normalizer (a high-shelf and a 60 Hz high-pass, all channels weighted equally), so bass-heavy and bright material are weighed roughly the way ears do, not by raw RMS;
- is smoothed with a time constant of 1.5 seconds, so it follows the music without jumping, and skips silence (below −70 LUFS);
- integrates only while the stage is fully on: not during the fade in or out, not while a flavour or plan swap is waiting (the wet and dry signals are mixed during a fade, which would corrupt the reading);
- starts over whenever any analog setting changes (the level has changed), so a reading needs about a second of audio, and the app trusts it after two;
- also tracks the output peak (decaying about 6 dB per second).

The numbers are **relative** (LUFS-like), good for comparing before and after, not calibrated to a broadcast standard.

**In the panel** (a "Level meter" block above the two slots):

- *Before*, *After*, *Change* and *Peak* for the slot you are hearing, with the change coloured green (within 0.5 dB), amber (within 1.5 dB) or red, and a marker on a −6 to +6 dB scale.
- A warning when the output peak reaches full scale ("may clip").
- When there is no reading it says why: the slot is dry (its change is 0 dB by definition), nothing is playing, or it is still measuring.
- For each slot, the level change last measured while you listened to it. A slot's measurement is forgotten when you edit it.

**Matching.** Play music with A for a few seconds, then with B. When both are measured, **Match B to A** (or **Match A to B**) sets the Output slider of that slot so that it comes out as loud as the other: it adds the difference to the current Output, rounded to the slider's half-dB steps and limited to ±6 dB. It says what it did, and if it hit the limit. The result is carried over so you can press it again without re-measuring; play a few seconds to confirm.

**Limits.**

- It measures the music you are playing. Different passages give slightly different differences (a bass note moves it more than a cymbal). Match on the kind of material you will listen to.
- Half-dB steps mean a match is within about a quarter of a decibel, which is small but not nothing for a blind test.
- It measures loudness, not "how it sounds": two equally loud versions can still feel different in level because of dynamics and tone.
- It works on the shared PCM output only (the stage does not run on DoP or bit-perfect output).
- CPU cost was not measured separately; it adds two K-weighting filter pairs per sample while the stage is on.

**Tests.** The meter follows amplitude (+6.02 dB for double, and the K-weighting shape), ignores silence, and starts fresh when reset; the engine's reading agrees with an independent offline measurement, follows a −6 dB Output trim within 0.15 dB, shows no change at mix 0, and reports nothing when the stage is off; the store records readings only with enough audio, rejects readings older than the last change, forgets on edit, and matches with the right arithmetic and limits; the panel shows all of it. While building it I found and fixed a real bug: the reading included the stage's fade-in, which made the first second wrong.

### 13.8 Blind test (ABX)

Code: [stores/abx.ts](../../player/ui/src/stores/abx.ts) and the "Blind test" block in [AnalogSection.vue](../../player/ui/src/components/AnalogSection.vue). It needs no engine change: hearing X simply sends the hidden slot's settings, exactly as switching to that slot would.

**How it works**

1. Set up A and B (a recipe, or your own), play music with A and then B, and press **Match B to A** so the levels agree.
2. Choose the number of **trials** (5, 10, 15 or 20) and press **Start blind test**.
3. A and B stay known. **X** is secretly one of them, picked at random for each trial. Switch among **A**, **B** and **X** as often as you like (buttons, or the keys A, B and X: X now means "hear X").
4. Answer **X is A** or **X is B**. The next trial starts on X with a new random pick.
5. At the end you get the score and the chance of doing that well by guessing (a one-sided binomial test at 50%), a plain-language verdict, and a list of every trial showing what X really was.

**Fairness rules**

- **The test will not start unless the two slots differ and the measured levels agree within 0.5 dB.** If they do not, it says why and points you to Match B to A. A checkbox, "Start anyway (results will be unreliable)", lets you override it.
- **Nothing gives X away while a test runs.** The panel hides the slot settings, the level meter, the engine status line (which names the plan) and the listening suggestions; it never shows which slot is playing. The shortcut messages say only "Blind test: hearing X" (or A or B). The slots cannot be edited during a test. The hidden choice is not held in the reactive state the panel reads.
- **The random choice** uses the browser's random source, a fair coin per trial.

**Reading the result.** With 10 trials, 9 right has about a 1% chance by guessing and 8 right about 5%, so 9 or 10 is convincing and 7 or fewer is not. A test that "cannot show you can hear the difference" does not prove you cannot; it means this run was not enough. More trials, more careful listening (quiet room, the recommended material, a passage that shows the effect) and higher drive settings all help.

**Not done**

- No automatic check that the level match holds for the passage playing during the test (the meter measures the music you played earlier).
- The result is not saved; close it and it is gone.
- It does not pause or restart the music for you: keep a passage looping, or start it before the test.

---

## 14. Phase 3 results

Code: the sag envelope, transformer stage and their settings in [dsp/analog/](../../crates/kahawai-player-core/src/dsp/analog/) (`shaper.rs`, `stage.rs`, `settings.rs`). Two new controls, **Sag** and **Transformer** (0 to 100%, default 30% each), in the engine settings, in `dsp.analog`, and in each A/B slot in Settings.

### 14.1 Sag (3.1)

- **What it models:** a tube stage's power supply droops when the signal is loud, so headroom shrinks and the stage plays a little quieter, then the supply recovers.
- **How:** a per-channel envelope follows the *driven* level (drive times the input): 5 ms attack, 120 ms release. The envelope, squashed by `e / (1 + e)` and scaled by the Sag setting, does two things: it drives the curve up to 50% harder (less headroom) and lowers the stage's output by up to 20%. Small signals barely move it; the effect is proportional to how loud and how driven the signal is.
- **Level match:** the auto gain now includes the steady-state sag at its reference level.
- **Tests:** with sag on, loud material is compressed more than quiet material (at least 0.5 dB more), and the stage is quieter just after a loud burst than a fraction of a second later (recovery), while with sag at 0 there is no such curve.

### 14.2 Transformer colour (3.2)

- **What it models:** an output transformer's core saturating at low frequencies and high levels, which adds bass harmonics without touching the mids or highs.
- **How:** the wet signal's bass (a one-pole low-pass at 90 Hz) is soft-clipped with a hard `tanh` and blended back in by the Transformer amount: `y = wet + amount × (softclip(bass) − bass)`. At amount 0 this is exactly the signal (bit-identical to Phase 2).
- **Tests:** at 59 Hz the 3rd harmonic is at least 8 dB higher with the transformer on, at least 12 dB higher at a high level than at a low level, and higher at a larger amount; at 1 kHz it changes by less than 3 dB.
- **Not tried:** a real hysteresis (Jiles-Atherton) model. This cheap version gives the level- and frequency-dependent bass colour; a hysteresis state would add memory effects. Left as a possible follow-up only if listening says it is missing.

### 14.3 Tone and linearity (3.3)

- **Linear response** is checked: with sag and transformer at 30%, drive at 0 and a small signal, both flavours stay within ±1 dB from 30 Hz to 16 kHz. (Below 30 Hz the 10 Hz coupling high-pass starts to show, by design.) So colour appears with level, not as a fixed tone change.
- **HF roll-off:** a per-flavour high-frequency roll-off would only matter at 96 kHz and above, and it would work against the goal of not changing the tone at low level. Not added.
- **Coupling high-pass:** the existing 10 Hz DC blocker; a per-flavour corner was not added.

### 14.4 CPU

Release build, one second of stereo audio, one core, with sag and transformer active (they add per-sample work: an envelope follower, a low-pass and a `tanh`):

| Sample rate, plan in use | Existing EQ | Triode | Solid state |
|---|---|---|---|
| 44.1 kHz (4x + ADAA) | 0.2% | 3.5% | 4.0% |
| 96 kHz (2x + ADAA) | 0.4% | 4.3% | 4.8% |
| 192 kHz (1x + ADAA) | 0.8% | 2.4% | 3.1% |

Phase 2 was 3.4%, 4.0% and 1.0% for the triode. The 192 kHz figure rose most because the added per-sample work is a larger share when no oversampling filter dominates. Still small. (Note: the earlier Phase 2 cost tables in sections 11 and 12 are unchanged history; output for every plan is in [PHASE1_COST.txt](../../research/analog-spike/PHASE1_COST.txt). The cost tool now sets the anti-aliasing plan explicitly; after the A/B panel added the `antialias` setting, it briefly measured "auto" for every row.)

### 14.5 What to listen for

- **Sag:** with drive up, loud hits should feel slightly softer and give way, and the body should come back a moment later. At 0% it should disappear.
- **Transformer:** on bass-heavy material (kick, bass guitar, organ), more weight and grit on loud bass notes at higher settings; nothing on quiet passages or on vocals.
- **A/B both:** put the same flavour in A and B with different sag and transformer amounts, or compare against 0% for both.

### 14.6 Still open

- Push-pull and hard-transistor flavours as extra presets.
- UI polish: a keyboard shortcut and a blind-test mode for the A/B panel.
- Whether the defaults (30% sag, 30% transformer) suit the music you play; they are guesses.
- Datasheet tuning of the triode curve, and published harmonic measurements.

---

## 15. More flavours: tube variants, push-pull, hard transistor

The Flavour menu now offers **ten** characters (Settings, Analog warmth, in each A/B slot). Choosing one also sets **Sag** and **Transformer** to typical values for that kind of stage (you can adjust them afterwards). Code: [dsp/analog/](../../crates/kahawai-player-core/src/dsp/analog/) (`settings.rs` lists the flavours, `models.rs` builds their curves); labels and typical values in [types.ts](../../player/ui/src/types.ts) (`FLAVOUR_INFO`).

### 15.1 The list

| Flavour (menu) | What it is | Sag / transformer |
|---|---|---|
| 12AX7 · high-mu preamp triode | The Phase 2 default (Koren's original fit) | 30% / 20% |
| 12AT7 (ECC81) · medium-high mu | Small-signal triode | 15% / 0% |
| 12AU7 (ECC82) · low mu, clean | Small-signal triode, mildest colour | 15% / 0% |
| 6SN7 · low-mu octal triode | Line-stage triode | 15% / 0% |
| 6DJ8 (ECC88) · low-noise triode | Medium mu, with contact potential | 15% / 0% |
| 300B · single-ended power triode | Power stage into a 3.5 kΩ transformer load | 40% / 50% |
| 2A3 · single-ended power triode | Power stage into a 2.5 kΩ transformer load | 40% / 50% |
| Push-pull tubes (2A3 pair) | Two 2A3 stages, outputs subtracted | 50% / 50% |
| Solid state · soft clip | Symmetric `tanh` (Phase 1) | 0% / 30% |
| Hard transistor · near-hard clip | Symmetric near-hard clip | 0% / 0% |

Small-signal tubes get no transformer by default (a preamp stage has none); the power stages and push-pull get one. These pairings are my guesses.

### 15.2 How the tubes are modelled

- **Parameters** are Koren's datasheet fits, verified in his tube library file ([Koren_Tubes.txt](https://polonai.se/audiofreaks/Koren_Tubes.txt), the LTspice library derived from his Glass Audio 1996 article; the 12AX7 line is his original fit and the others are per-datasheet fits):

| Tube | mu | Ex | Kg1 | Kp | Kvb | Vct | Source noted in the library |
|---|---|---|---|---|---|---|---|
| 12AX7 | 100 | 1.4 | 1060 | 600 | 300 | 0 | Koren's original |
| 12AT7 | 67.49 | 1.234 | 419.1 | 213.96 | 300 | 0 | Tom Mitchell |
| 12AU7 | 20.21 | 1.230 | 1108.7 | 84.96 | 551.3 | 0 | Sylvania Technical Manual |
| 6SN7 | 21.07 | 1.341 | 1446.2 | 157.81 | 179.4 | 0 | Sylvania Technical Manual |
| 6DJ8 | 30.51 | 1.532 | 453.9 | 233.17 | 190.9 | 0.5 | Tom Mitchell |
| 300B | 3.92 | 1.504 | 2140.3 | 64.28 | 300 | 0 | Western Electric, 1950 |
| 2A3 | 4.05 | 1.634 | 3652.2 | 58.47 | 300 | 0 | Tung-Sol datasheet |

  The equation is the library's `TRIODE` subcircuit (plate current from `E1 = Vpk/Kp · ln(1 + exp(Kp(1/mu + (Vgk + Vct)/√(Kvb + Vpk²))))`, twice `E1^Ex / Kg1`), including the contact potential the newer library added. I left out the library's "12AX7A" (RCA) fit: its Kp is 17950, a different scale from the others, and I did not want to use it without understanding why.
- **The stage around the tube** sets the operating point: resistor-loaded for the small-signal tubes (supply and load in the table below, grid bias chosen to put the plate at half the supply, except the 12AX7 which keeps its fixed −1.5 V), and a stated operating point into an AC load for the power tubes. The curve is then the plate voltage along the *AC load line through that point*, which is how a transformer-coupled stage behaves.
- **Input scale:** for every tube, one unit of curve input is a grid swing equal to the bias, so `u = 1` reaches 0 V on the grid and grid conduction starts there (as in Phase 2). That keeps Drive comparable across tubes.
- **Results (from the code):**

| Tube | Supply / load | Bias | Plate | Current |
|---|---|---|---|---|
| 12AX7 | 300 V / 100 kΩ | −1.5 V | 205 V | 1.0 mA |
| 12AT7 | 250 V / 47 kΩ | −1.5 V | 125 V | 2.7 mA |
| 12AU7 | 300 V / 47 kΩ | −6.8 V | 150 V | 3.2 mA |
| 6SN7 | 300 V / 47 kΩ | −5.4 V | 150 V | 3.2 mA |
| 6DJ8 | 200 V / 22 kΩ | −2.8 V | 100 V | 4.5 mA |
| 300B | 300 V, 65 mA, 3.5 kΩ AC | −60.0 V | 300 V | 65 mA |
| 2A3 | 250 V, 60 mA, 2.5 kΩ AC | −44.2 V | 250 V | 60 mA |

  **A real cross-check:** for the 300B the model puts the bias at −60 V for 300 V and 65 mA; published operating points for that tube are around −62 V. For the 2A3 it gives −44 V at 250 V and 60 mA, against the usual −45 V (both from general knowledge, not a sourced datasheet in this pass). Tests assert both within a few volts. The small-signal stages have no such published check yet.

### 15.3 Push-pull and hard transistor

- **Push-pull (2A3 pair):** the second tube sees the inverted signal and the two outputs are subtracted: `y = (g(u) − 0.92·g(−u)) / 1.92`, with `g` the 2A3 curve. That cancels the even harmonics of the single tube (about 15 dB or more better in the test) and leaves the odd ones; the 8% mismatch keeps a small even remainder, as real pairs have. Class-A push-pull only; no crossover model.
- **Hard transistor:** `y = u / (1 + |u|⁸)^(1/8)`: linear below about 0.8, flat above about 1.2, with a small rounded corner. Clean, then harsh.

### 15.4 What the measurements say (drive 40%, sag and transformer 0, 1 kHz)

Harmonics relative to the tone, at an input level of 0.1 / 0.3 / 0.6:

| Flavour | 2nd (dB) | 3rd (dB) |
|---|---|---|
| 12AX7 | −36 / −26 / −25 | −66 / −46 / −24 |
| 12AT7 | −37 / −27 / −30 | −79 / −61 / −27 |
| 12AU7 | −36 / −27 / −29 | −79 / −62 / −26 |
| 6SN7 | −41 / −32 / −51 | −75 / −55 / −26 |
| 6DJ8 | −41 / −32 / −56 | −71 / −50 / −25 |
| 300B | −46 / −36 / −51 | −70 / −50 / −24 |
| 2A3 | −44 / −34 / −40 | −69 / −49 / −24 |
| Push-pull | −72 / −61 / −68 | −69 / −49 / −24 |
| Solid state | none | −49 / −30 / −20 |
| Hard transistor | none | −135 / −59 / −21 |

- **Single-ended tubes** all show even-dominant harmonics at moderate levels (the 2nd is at least 15 dB above the 3rd in the test) and turn odd-heavy as they are overdriven (0.6), when the tube clips against cut-off and grid conduction. The differences between tubes are modest: the low-mu tubes and the power triodes are cleaner at a given input, the 12AX7 and 12AT7 the most coloured.
- **Push-pull** has almost no 2nd harmonic and the same 3rd as the 2A3 alone; the odd harmonics take over.
- **Hard transistor** is essentially clean below the knee, then the 3rd jumps from −135 to −59 to −21 dB.
- **Aliasing** at the hard test tone with the automatic plan is below −70 dB for every flavour at 44.1 kHz and below −72 dB at 96 and 192 kHz (tested at −65 dB or better).

### 15.5 Caveats

- **Static curves.** The tubes are modelled as static transfer curves plus the sag and transformer stages. There is no interelectrode capacitance (so no high-frequency roll-off or Miller effect), no cathode bypass or coupling dynamics, and no hum or noise.
- **The stages around the tubes are typical, not real amplifiers.** Supply voltages, loads and transformer loads are common values, not from any specific product.
- **Datasheet fits are approximations** away from their fitting region, and the fits are Koren's and others', not my own measurements.
- **Not listened to.** All of this is measured, not heard. The typical sag and transformer values are guesses.
- **Names:** the menu uses the tubes' type numbers (and their common European equivalents) as descriptions of a modelled stage, not as endorsements or claims about specific brands.

---

## 16. Eleven more flavours (21 in total)

Seven more tubes and pentode pairs, three solid-state characters, and one utility. Menu order: small-signal tubes, power triodes, the pentode, push-pull, solid state, then the utility. Code: [dsp/analog/models.rs](../../crates/kahawai-player-core/src/dsp/analog/models.rs); menu text and typical Sag/Transformer in [types.ts](../../player/ui/src/types.ts).

### 16.1 New tubes (Koren's library fits)

| Flavour | Tube fit (library note) | mu | Ex | Kg1 | Kp | Kvb | Vct / screen | Operating point (code) |
|---|---|---|---|---|---|---|---|---|
| 12AX7A (Sylvania) | Sylvania technical manual, 1955 | 105.78 | 1.474 | 1618.2 | 432.76 | 35.6 | Vct 0.5 | 300 V / 100 kΩ, bias −0.8 V, plate 150 V |
| 12AY7 | GE databook, 1955 | 44.16 | 1.113 | 1192.4 | 409.96 | 300 | Vct 0 | 300 V / 100 kΩ, bias −2.5 V, plate 150 V |
| 6SL7GT | GE | 75.89 | 1.233 | 1735.2 | 1725.27 | 7.0 | Vct 0.5 | 300 V / 100 kΩ, bias −1.2 V, plate 150 V |
| EL84 (single-ended) | Mullard | 21.29 | 1.240 | 401.7 | 111.04 | 17.9 | screen 250 V | 250 V, 48 mA, 5.2 kΩ AC, bias −7.6 V |
| EL34 pair (class AB) | Mullard data book, 1962 | 12.02 | 1.169 | 353.9 | 61.11 | 29.9 | screen 400 V | 400 V, 35 mA idle each, 1.65 kΩ, bias −35.9 V |
| 6L6GC pair (class AB) | GE data sheet | 9.88 | 1.442 | 1686.6 | 30.98 | 19.4 | screen 400 V | 400 V, 35 mA idle, 1 kΩ, bias −42.9 V |
| KT88 pair (class AB) | M-O Valve | 12.38 | 1.246 | 340.4 | 26.48 | 36.5 | screen 400 V | 450 V, 50 mA idle, 1 kΩ, bias −50.1 V |

Parameters are the lines in the same library file as section 15. The three pentode and beam-tetrode fits use the library's "PENTODE1" model: `E1 = Vg2/Kp · ln(1 + exp(Kp (1/mu + Vg1/Vg2)))`, `Ip = 2 · E1^Ex / Kg1 · atan(Vp / Kvb)`, with the screen held at a fixed voltage and its current ignored.

- **EL84 cross-check:** for the published single-ended class-A operating point (250 V, screen 250 V, 48 mA) the model's bias is −7.6 V against the published −7.3 V; a test holds it within 1 V. The other pentode operating points are typical values and have no such check.
- **Class AB:** the pair is two of the same tube on opposite half-cycles, biased near cutoff at the stated idle current, and the output is the *difference of the two plate currents*. Where the two tubes hand over there is a crossover region, so the 3rd harmonic is already present at low level. A 2% mismatch between the tubes leaves a little even harmonic.

### 16.2 Solid state and utility

| Flavour | What it is |
|---|---|
| JFET | Square-law transfer `Id = Idss (1 − Vgs/Vp)²`, biased at half pinch-off, cut off on one side and clipped by gate conduction on the other. Nearly pure 2nd harmonic |
| Silicon diode clipper | Antiparallel diode pair: `asinh(3u)/3`, a symmetric logarithmic soft clip |
| Germanium diode clipper | A germanium diode against a silicon one: `asinh(6u)/6` on the positive half, `asinh(3u)/3` on the negative: lopsided |
| Transformer and sag only | A linear curve: no tube or transistor colour, only the supply sag and the transformer's bass saturation |

The JFET, diode and iron curves are simple closed forms, not fits to particular devices.

### 16.3 Harmonics (drive 40%, sag and transformer 0, 1 kHz; input 0.1 / 0.3 / 0.6)

| Flavour | 2nd (dB) | 3rd (dB) | Note |
|---|---|---|---|
| 12AX7A (Sylvania) | −45 / −36 / −37 | −79 / −59 / −28 | cleaner than the original 12AX7 fit |
| 12AY7 | −44 / −35 / −41 | −72 / −52 / −26 | |
| 6SL7GT | −57 / −48 / −29 | −93 / −74 / −29 | very clean, then abrupt |
| EL84 (SE class A) | −33 / −40 / −32 | −44 / −22 / −14 | both kinds; the 3rd dominates at 0.3 |
| EL34 pair | −54 / −49 / −53 | −40 / −37 / −14 | crossover 3rd from low level |
| 6L6GC pair | −57 / −49 / −49 | −42 / −27 / −26 | |
| KT88 pair | −56 / −48 / −50 | −37 / −24 / −19 | |
| JFET | −26 / −16 / −12 | −158 / −99 / −23 | square law: 3rd absent until it clips |
| Silicon diodes | none | −37 / −24 / −19 | symmetric |
| Germanium diodes | −30 / −23 / −22 | −31 / −21 / −18 | lopsided: both kinds |
| Transformer and sag only | none | none (−157) | linear |

(The push-pull pentode rows include the 2% tube mismatch, which leaves the small 2nd harmonic; a perfectly matched pair would show none.)

### 16.4 Aliasing and cost

With the automatic plan on the hard test tone: every flavour is below −60 dB at 44.1 kHz and 192 kHz in the tests; measured across all of them, the worst are the class-AB pentode pairs (about −63 dB at 44.1 kHz, −68 dB at 96 and 192 kHz) because their curves have steeper corners; the rest are −67 to −125 dB. CPU is the same as any table flavour (the curve is a lookup); the pentode tables take about the same 20 ms to build on first use (per flavour).

### 16.5 Caveats

- Same as section 15.5: static curves, typical (not real) stages, datasheet fits used outside their fitting region, nothing listened to.
- The pentode and class-AB stages ignore the screen grid current, the screen supply sag, the output transformer's real reflected load, and negative feedback. Real amplifiers differ, often a lot; these are characters, not amplifier simulations.
- The JFET, diode and iron flavours are idealised.

---

## 17. Flavour guide (for anyone new to analog tubes)

### 17.1 Terms in plain language

- **Triode, pentode:** kinds of vacuum tube. A triode has three parts inside (cathode, grid, plate) and a smooth, gradual response. A pentode adds two more grids and gives more power and a rougher, brighter sound. A *beam tetrode* (6L6, KT88) behaves like a pentode.
- **Small-signal (preamp) tube:** a tube used to boost a weak signal, such as the 12AX7. **Power tube:** one that drives a speaker, such as the 300B, EL34 or KT88.
- **Gain (mu):** how much a tube amplifies. High-mu tubes (12AX7, mu about 100) amplify a lot; low-mu tubes (12AU7, mu about 20) amplify less and stay cleaner.
- **Harmonics:** extra notes an imperfect circuit adds at whole-number multiples of each frequency. The **2nd harmonic** is an octave up and sounds warm and full; the **3rd** is an octave and a fifth up and sounds harder and more edgy; higher ones sound harsh. **Even** harmonics (2nd, 4th) are associated with the tube "warmth", **odd** ones (3rd, 5th) with a firmer or grittier sound.
- **Single-ended vs push-pull:** a single-ended stage uses one tube and is lopsided (lots of 2nd harmonic). A push-pull stage uses two tubes working on opposite halves of the wave, which cancels the even harmonics and leaves the odd ones, and can deliver more power.
- **Class A vs class AB:** in class A a tube conducts all the time: smooth at every level. In class AB (used in push-pull power stages) each tube rests near cut-off and hands over to the other one near zero, which can leave a small rough patch at very low levels ("crossover distortion").
- **Sag:** in a real amplifier, loud passages pull the power supply down for a moment, which softens the peaks and then recovers. It gives tube amps a "breathing" feel.
- **Transformer colour:** output transformers saturate at the bottom of the frequency range when driven hard, which thickens and slightly distorts the bass without touching the mids and highs.
- **Drive, mix:** how hard the signal is pushed into the effect, and how much of the effect is blended with the untouched signal.
- **Aliasing, oversampling, ADAA:** digital distortion creates frequencies that the sampled signal cannot hold, and they fold back as harsh, unrelated tones (aliasing). Oversampling (processing at a higher rate) and ADAA (a mathematical correction) keep that out. You should not hear it working.

### 17.2 The flavours, in menu order

| Flavour | What it is like |
|---|---|
| 12AX7 | The classic guitar-amp and preamp tube. Soft, even-harmonic warmth. The default. |
| 12AX7A (Sylvania) | Another measured 12AX7: a touch more even harmonic at low levels. |
| 12AT7 (ECC81) | A little cleaner than the 12AX7, firmer and more open. |
| 12AU7 (ECC82) | Low gain, low distortion. The mildest tube colour. |
| 12AY7 | Between the 12AU7 and 12AT7: clean, with a gentle lift in the 2nd. |
| 6SN7 | Smooth, full-bodied; a favourite for line stages in hi-fi. |
| 6SL7GT | Very clean until pushed, then turns over abruptly. |
| 6DJ8 (ECC88) | Taut and detailed, a low-noise tube. |
| 300B | The classic single-ended power triode: rich 2nd harmonic, gentle overload, transformer and sag. |
| 2A3 | Like the 300B, a little lighter and quicker. |
| EL84 (single-ended) | A class-A pentode: brighter and grittier than a triode, with both even and odd harmonics. |
| Push-pull tubes (2A3 pair) | Fuller and firmer: even harmonics cancel, odd ones and compression take over as it is driven. |
| Push-pull EL34 | The British power stage: odd harmonics, firm compression, a touch of low-level grit. |
| Push-pull 6L6GC | The American power stage: cleaner and stiffer than EL34s. |
| Push-pull KT88 | Big, tight, high-power: plenty of headroom. |
| Solid state (soft clip) | Symmetric and clean with a soft odd-harmonic edge, like a transformer-coupled console preamp. |
| JFET | Nearly pure 2nd harmonic, almost no 3rd: a very smooth warmth. |
| Silicon diode clipper | A logarithmic soft clip: odd harmonics that build gradually. |
| Germanium diode clipper | One half of the wave clips earlier than the other: rougher, fuzzier. |
| Hard transistor | Clean until it clips, then harsh. For effect, not fidelity. |
| Transformer and sag only | No distortion curve: just the transformer's bass colour and the supply sag. |

These descriptions are the same one-liners shown under the Flavour menu in Settings. They describe the *modelled stage*, not a specific product.

---

## 18. Listening suggestions

Ten ready-made comparisons. In Settings, under **Analog warmth**, open **Listening suggestions** and press **Set up A and B**: it loads both slots and starts you on A. Then play the suggested music and switch.

**Three rules for a fair comparison**

1. **Match the level.** Use the Output slider (plus or minus 6 dB) until A and B are equally loud. The louder one nearly always sounds better, and this is the most common way to fool yourself.
2. **Switch while the music plays,** on the same passage, and listen for a few seconds each way. Do it several times.
3. **Trust quiet, careful listening over big settings.** If you cannot hear a difference, raise Drive and Mix to hear what the effect *is*, then bring them back. For a real test, use the built-in **blind test** (section 13.8): it hides which is which, and it will not start until the levels match.

| # | Recipe | A | B | Play | Listen for |
|---|---|---|---|---|---|
| 1 | Warmth against nothing | Off (dry) | 12AX7, drive 50%, mix 50% | Acoustic guitar, piano or a solo voice | A touch more body and sheen, no change in loudness |
| 2 | Pure 2nd harmonic: JFET against 300B | JFET, drive 60%, mix 70% | 300B, drive 60%, mix 70% | A male voice or cello | JFET: smooth, clean-sounding fullness. 300B: richer and heavier, softening on loud notes |
| 3 | British against American power stages | EL34 pair, drive 60%, mix 70% | 6L6GC pair, drive 60%, mix 70% | Drums, electric guitar or bass, fairly loud | EL34: warmer, more compressed, a little gritty. 6L6GC: cleaner, tighter, firmer low end |
| 4 | Class A against class AB at low level | 2A3 push-pull, drive 60%, mix 100% | EL34 pair, drive 60%, mix 100% | A quiet, sparse passage: brushed drums, a solo instrument, a fade-out | Class AB adds a rough, buzzy edge to soft notes that class A does not |
| 5 | How much drive? | 12AX7, drive 30%, mix 50% | 12AX7, drive 70%, mix 50% | A full mix with dynamics | 30%: a light glow. 70%: loud passages soften and thicken; watch for fatigue |
| 6 | Small-signal tubes: clean against colourful | 12AU7, drive 70%, mix 60% | 12AX7, drive 70%, mix 60% | Vocals and acoustic instruments | 12AU7: subtle, transparent. 12AX7: livelier, more sheen on voices |
| 7 | Sag and transformer on drums and bass | Transformer and sag only, both 0%, mix 100% | Transformer and sag only, both 80%, mix 100% | Kick drum and bass, loud | Loud kicks give way slightly and bloom back; bass gains weight and grit. Off should sound like the source |
| 8 | Soft clip against hard clip | Solid state, drive 80%, mix 80% | Hard transistor, drive 80%, mix 80% | A loud, dense track; cymbals and distorted guitars | Soft: thickens and rounds. Hard: harsh and buzzy as soon as it clips |
| 9 | Symmetric against lopsided clipping | Silicon diodes, drive 70%, mix 80% | Germanium diodes, drive 70%, mix 80% | An electric guitar or synth lead, loud | Silicon: even and smooth. Germanium: rougher, fuzz-like |
| 10 | Does anti-aliasing matter? | 12AX7, drive 100%, mix 100%, 1x no protection | Same, 4x + ADAA | Bright material at 44.1 or 48 kHz: cymbals, hi-hats, high piano (turn the volume down first) | Without protection, a fine metallic fizz that is not in the music. With it, a clean top end. (At 96 kHz and above the difference is small.) |

Notes:

- Recipe 1 is the best first test. Recipe 10 is a good way to hear what the engineering in sections 12 to 14 does.
- Sag and Transformer take each flavour's typical values (section 15); the recipes only override them in recipe 7.
- The recipes and the button live in the app ([types.ts](../../player/ui/src/types.ts), `LISTENING_RECIPES`); a test checks that every recipe title appears in this section.

---

## 19. References

How each was used: **read** = read in full or in its main parts; **summary** = I read a summary or excerpt only (not the whole paper); **search** = found in search results and not opened. Reading depth matters: claims that lean on a "summary" or "search" entry should be confirmed before they are relied on.

### Tube models

- Norman Koren, "Improved vacuum tube models for SPICE simulations" (Glass Audio, 1996): [part 1](https://www.normankoren.com/Audio/Tubemodspice_article.html) (read: equations and the 12AX7, 12AU7 and 6L6CG parameter table), [part 2](https://www.normankoren.com/Audio/Tubemodspice_article_2.html) (read: the 12AX7 and 6550 parameter lines).
- [Koren's tube library](https://polonai.se/audiofreaks/Koren_Tubes.txt) (read: the datasheet-fitted parameters for every tube in sections 15 and 16, and the `TRIODE` and `PENTODE1` equations; an LTspice-format library derived from his work, hosted by a third party).
- [Nonlinear SPICE models of vacuum-tube triodes, Electronic Design](https://www.electronicdesign.com/technologies/analog/article/55246421/modeling-on-mondays-nonlinear-spice-models-of-vacuum-tube-triodes-part-3) (search).
- [Measures and models of real triodes, for the simulation of guitar amplifiers](https://www.researchgate.net/publication/281075913_Measures_and_models_of_real_triodes_for_the_simulation_of_guitar_amplifiers) (search).
- [A quadric surface model of vacuum tubes for virtual analog (DAFx 2023)](https://dafx.de/paper-archive/2023/DAFx23_paper_15.pdf) (search).

### Wave digital filters and circuit simulation

- [New family of wave-digital triode models (Aalto)](https://aaltodoc.aalto.fi/bitstreams/c94fca6e-463e-4f8e-a25d-9dce4aa4e8c8/download) (search).
- [Enhanced wave digital triode model for real-time tube amplifier emulation (Pakarinen and Karjalainen, IEEE)](https://ieeexplore.ieee.org/document/5272282/) (search).
- [A Csound opcode for a triode stage of a vacuum tube amplifier (DAFx 2011)](http://recherche.ircam.fr/pub/dafx11/Papers/42_e.pdf) (search).
- [RT-WDF: a modular wave digital filter library (DAFx)](https://www.dafx.de/paper-archive/details/ilZF4akpmSxzyiusoAOolg) (search; the [repository](https://github.com/RT-WDF/rt-wdf_lib): metadata read, no license file, last pushed 2017).
- [libmksim](https://github.com/mkaudio-company/libmksim) (read: README summary and repository metadata; MIT, created 2026-03-08).

### Antialiasing

- Julian Parker, Vadim Zavalishin and Efflam Le Bivic, "Reducing the aliasing of nonlinear waveshaping using continuous-time convolution" (DAFx-16) (search), with the [companion code](https://github.com/julian-parker/DAFX-AntiAliasing) (metadata only; no license).
- Stefan Bilbao, Fabián Esqueda, Julian Parker and Vesa Välimäki, "Antiderivative antialiasing for memoryless nonlinearities" (IEEE Signal Processing Letters, 2017) (search).
- [Antiderivative antialiasing for stateful systems (DAFx 2019)](https://www.hsu-hh.de/ant/wp-content/uploads/sites/699/2020/10/DAFx2019_paper_4.pdf) (search).
- [Jatin Chowdhury's ADAA experiments](https://github.com/jatinchowdhury18/ADAA) (metadata only; BSD-3-Clause), and his article [Practical considerations for antiderivative anti-aliasing](https://jatinchowdhury18.medium.com/practical-considerations-for-antiderivative-anti-aliasing-d5847167f510) (search; the page returned an access error when opened).

### Transformers and other approaches

- [A transformer model based on the Jiles-Atherton theory of ferromagnetic hysteresis](https://www.researchgate.net/publication/270465159_A_transformer_model_based_on_the_Jiles-Atherton_theory_of_ferromagnetic_hysteresis) (search).
- [Real-time audio transformer emulation for virtual tube amplifiers](https://www.researchgate.net/publication/220057543_Real-Time_Audio_Transformer_Emulation_for_Virtual_Tube_Amplifiers) (search).
- [Deep learning for tube amplifier emulation](https://arxiv.org/pdf/1811.00334) (search).

### Not sourced here

- **Published operating points** used as cross-checks (300B about −62 V at 300 V and 65 mA; 2A3 about −45 V at 250 V and 60 mA; EL84 single-ended −7.3 V at 250 V, 250 V screen and 48 mA) come from general knowledge of standard datasheet operating points, not from a link in this list. They are worth checking against the manufacturers' datasheets.
- **Typical stage values** (supply voltages, load resistances, idle currents, transformer loads) are common textbook values, not from a particular amplifier.
- **The JFET, diode and hard-clip curves** are idealised closed forms.
- **Harmonic characteristics of tube stages** (even against odd, class A against class AB) are standard audio-electronics background, confirmed here only by our own measurements of the models (sections 10 to 16).
