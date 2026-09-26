# Analog emulation: tube and transistor "euphonics"

Research summary and an itemized plan for adding tube and transistor
character to the PCM playback chain. Branch: `analog-poc`. Status: **Phase 0 (research) and Phase 1 (working stage in the engine,
no UI yet) done.** Findings are in section 10, Phase 1 results in section 11.

Written for the Kahawai maintainers. Related: [Backlog.md](Backlog.md)
(DSP effects section), [EQ.md](EQ.md) (pipeline and the `DspStage` idea).

How to read the claims below:

- **[cited]** comes from a source in section 9, found in this research pass.
  I read the search summaries, not the full papers, so treat details as
  leads to confirm in Phase 0.
- **[known]** is general audio-DSP background I am confident of but did not
  source here.
- **[guess]** is my estimate and must be measured before it is relied on.

---

## 1. Goal and non-goals

**Goal:** an optional, subtle "warmth" stage in the shared PCM path that
can be switched among a few flavours (tube and transistor), with a drive
control and a dry/wet mix, sounding good on music at normal listening
levels and costing little CPU.

**Non-goals:**
- Not circuit-accurate emulation of a specific product. We model
  *character* (harmonics, level-dependent softness, a little dynamics), and
  name presets as flavours, not brands.
- Not a guitar-amp simulator (no tone stacks, cabinets, heavy clipping).
- Not on the exclusive paths: DoP and bit-perfect playback bypass it, as
  they do EQ, loudness and volume.

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

At music levels the audible effect is a **low-order harmonic colouring that
grows with level**, plus mild compression. Getting that right, without
aliasing, matters more than circuit detail.

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

A nonlinearity creates harmonics above the original band. Without
protection they fold back below Nyquist as inharmonic, harsh distortion.
Two families of fixes:

- **Oversampling** (2x to 8x around the nonlinear part, with good
  anti-alias and decimation filters). Robust and simple to reason about.
  Costs CPU in proportion to the factor, and adds latency with linear-phase
  filters (**[known]**).
- **Antiderivative antialiasing (ADAA)**, introduced by Parker, Zavalishin
  and Le Bivic (DAFx 2016) and developed by Bilbao, Esqueda, Parker and
  Välimäki (**[cited]**). It reduces aliasing without oversampling, for
  memoryless nonlinearities. It needs the antiderivative of the curve
  (closed form for tanh and hard clip). There are extensions to stateful
  systems (**[cited]**). Often combined with a small oversampling factor.

Our sources are 44.1 to 192 kHz. Content at 96 kHz and above has a lot of
headroom already, so the required oversampling depends on the source rate:
**[guess]** 4x at 44.1/48 kHz, 2x at 88.2/96 kHz, none or ADAA-only above.
Confirm by measuring aliasing (section 6).

### 3.3 Output transformer and power-supply sag

- Transformer saturation and hysteresis are usually modelled with the
  Jiles-Atherton theory (**[cited]**), which is accurate but implicit and
  hard to fit and run in real time (**[cited]**). Cheaper alternatives are a
  level-dependent low-shelf plus a soft clip, or a small hysteresis-like
  state (**[known]**). Start with the cheap version.
- Sag can be modelled as a slow (tens of milliseconds) envelope of the
  signal that lowers the stage's supply, which lowers headroom and gain
  (**[known]**). It is a control-rate effect and cheap.

### 3.4 Existing implementations (license matters)

- **libmksim** (Rust; WDF plus Newton solvers for tubes, diodes, transistors,
  op-amps; MIT). **Phase 0 verdict: reference at most, not a dependency.**
  The GitHub repository was created on 2026-03-08 and its four commits all
  landed within 11 minutes, it has 3 stars, is not on crates.io, and its
  README calls AVX-512 a placeholder. No track record.
- **RT-WDF** (C++ WDF library). **Verdict: skip.** No license file (GitHub
  shows none), last pushed in 2017, needs the Armadillo C++ library, and
  the material I could read does not show a triode example.
- **Antialiasing reference code:** Chowdhury's ADAA repository is
  BSD-3-Clause (usable as a reference); Parker's DAFX-AntiAliasing repository
  has no license (read, do not copy).
- Some well-known open-source emulations are GPL (**[known]**, not checked
  for any specific project). Rule from the Backlog: do not copy code from
  a project until its license is confirmed compatible.

---

## 4. Proposed design

### 4.1 Where it lives

- A new module `analog.rs` in
  [kahawai-player-core](crates/kahawai-player-core/src/) (pure Rust, no
  platform imports, like `dsp.rs`).
- A small trait so stages compose (this is the `DspStage` idea from
  EQ.md):

```rust
pub trait DspStage: Send {
    fn prepare(&mut self, sample_rate: u32, channels: u16);
    fn process(&mut self, interleaved: &mut [f32]);   // in place
    fn latency_frames(&self) -> u32 { 0 }
    fn reset(&mut self);
}
```

- Chain order in `pump_pcm` (see [engine.rs](crates/kahawai-player-core/src/engine.rs)):
  decode, resample, **EQ, analog stage**, loudness gain, volume, sink. The
  analog stage goes after the EQ so the user's EQ shapes what is driven,
  and before loudness and volume so drive is independent of listening level.
  (Open question Q3.)

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

Inspired-by references (for our own measurement targets only, not for the
UI): single-ended triode line stages, push-pull power stages, classic
transformer-coupled console preamps. Final list to be chosen in Phase 0.

---

## 5. Itemized plan

Sizes: **S** an hour or two, **M** a day, **L** several days. Each phase ends
with something we can listen to and a green test suite.

### Phase 0: research and decisions (M) *(this document is the start)*

| # | Item | Output |
|---|---|---|
| 0.1 | Read the key sources in section 9 in full (Koren tube models, ADAA, WDF triode, transformer emulation) | **Partly done.** Koren's equations and parameters read and used; ADAA basics implemented and measured. WDF and transformer papers not yet read in full |
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
| 1.6 | Engine command, settings persistence | Done (no UI, no snapshot field) |

### Phase 2: derive curves from tube models (M)

| # | Item | Notes / acceptance |
|---|---|---|
| 2.1 | Offline tool (a Rust test or example) that evaluates the Koren triode equations and produces a normalized transfer curve for a chosen tube and load line | Curve plot and a stored table per flavour |
| 2.2 | Replace the hand-made curves with table lookups (interpolated) for the tube flavours | THD profile within tolerance of the targets (0.2) |
| 2.3 | Add ADAA for the table curves (or a smooth closed-form fit with a known antiderivative) so the oversampling factor can drop | Aliasing and CPU comparison recorded |

### Phase 3: dynamics and transformer colour (M)

| # | Item | Notes / acceptance |
|---|---|---|
| 3.1 | Sag: slow envelope follower that lowers headroom and gain, with attack and recovery | Level-dependent gain drop matches the flavour target |
| 3.2 | Transformer colour: cheap level-dependent low shelf plus soft clip; evaluate a hysteresis state as a follow-up | Bass tone THD rises with level and falls with frequency, as intended |
| 3.3 | Tone shaping: coupling and roll-off filters per flavour | Frequency response within a stated tolerance |

### Phase 4: UI (M)

| # | Item | Notes / acceptance |
|---|---|---|
| 4.1 | "Warmth" button next to the EQ button, opening a modal in the EQ dialog's style | Same look, both themes |
| 4.2 | Flavour picker, Drive, Mix, Output; live preview with OK / Cancel, like the EQ | Cancel reverts |
| 4.3 | Dim with "not supported for this stream type" on exclusive paths | Same rule as the EQ |
| 4.4 | Guardrails: limit drive, warn when the stage will clip the output; gain-matched A/B toggle | Consistent with the EQ guardrails |
| 4.5 | Tests: component, store, and a house-style check | Green |

### Phase 5: optional higher fidelity (L, only if Phases 1-4 are not enough)

| # | Item | Notes |
|---|---|---|
| 5.1 | WDF-based triode stage (or libmksim) for one flavour | Compare against the table version by ear and measurement |
| 5.2 | Jiles-Atherton transformer for one flavour | Only if the cheap version is clearly lacking |
| 5.3 | Move the EQ, analog and future stages into a small chain editor | Backlog: "Effects chain UI" |

---

## 6. Verification plan

Everything is deterministic DSP, so most of it is testable in Rust without
listening. Listening tests still decide "does it sound good".

1. **Harmonic profile.** Feed a 1 kHz sine at several levels, take an FFT,
   and check the 2nd, 3rd and 5th harmonic levels versus drive against the
   flavour's targets (tolerance in dB).
2. **Aliasing.** Feed a high-frequency sine (for example 15 to 20 kHz at
   44.1 kHz) hard, and check that no inharmonic component rises above a
   threshold below the fundamental. Run at 44.1, 96 and 192 kHz. Compare
   oversampling factors and ADAA.
3. **Transparency.** With the stage off, or mix 0, the output equals the
   input bit for bit. At low drive and low levels, distortion stays below a
   stated figure.
4. **No clicks.** Change parameters and toggle the stage while a tone
   plays; measure the largest sample-to-sample jump, as in the EQ tests.
5. **Level and gain match.** Loudness of the wet and dry signal within
   about 1 dB at default settings.
6. **Path rules.** DoP and bit-perfect output are bit-identical with the
   stage enabled (as the existing DoP test does for the EQ).
7. **Performance.** Cost per chunk at 44.1 and 192 kHz stereo stays under a
   set share of the chunk's real-time duration (budget from item 0.4).
8. **Listening.** A/B on a fixed set of tracks (acoustic, vocal, dense rock,
   electronic) with gain matching, before merging each phase.

---

## 7. Open questions (decide in Phase 0)

- **Q1. Which stages to offer first?** All four flavours in 4.3, or one
  tube and one transistor flavour?
- **Q2. Oversampling or ADAA first?** Oversampling is simpler and more
  robust; ADAA saves CPU. Plan: oversampling in Phase 1, ADAA in Phase 2.
- **Q3. Where in the chain?** After the EQ and before loudness and volume
  (proposed), or after loudness? It changes how drive relates to level.
- **Q4. Latency.** Linear-phase oversampling filters add a fixed delay
  (a few milliseconds). Accept it and compensate in the playhead, or use
  minimum-phase filters?
- **Q5. Gain match.** Automatic (safer for A/B) or manual?
- **Q6. libmksim.** Dependency, reference, or ignore?
- **Q7. Naming and legal.** Preset names describe character; confirm we are
  comfortable with "inspired by" notes in documentation only.

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

Found in this research pass (search summaries read; confirm in full in
Phase 0.1):

- Koren, "Improved vacuum tube models for SPICE simulations":
  [part 1](https://www.normankoren.com/Audio/Tubemodspice_article.html),
  [part 2](https://www.normankoren.com/Audio/Tubemodspice_article_2.html)
- [Nonlinear SPICE models of vacuum-tube triodes (Electronic Design)](https://www.electronicdesign.com/technologies/analog/article/55246421/modeling-on-mondays-nonlinear-spice-models-of-vacuum-tube-triodes-part-3)
- [Measures and models of real triodes, for the simulation of guitar amplifiers](https://www.researchgate.net/publication/281075913_Measures_and_models_of_real_triodes_for_the_simulation_of_guitar_amplifiers)
- [A quadric surface model of vacuum tubes for virtual analog (DAFx 2023)](https://dafx.de/paper-archive/2023/DAFx23_paper_15.pdf)
- [New family of wave-digital triode models (Aalto)](https://aaltodoc.aalto.fi/bitstreams/c94fca6e-463e-4f8e-a25d-9dce4aa4e8c8/download)
- [Enhanced wave digital triode model for real-time tube amplifier emulation (IEEE)](https://ieeexplore.ieee.org/document/5272282/)
- [A Csound opcode for a triode stage of a vacuum tube amplifier (DAFx 2011)](http://recherche.ircam.fr/pub/dafx11/Papers/42_e.pdf)
- [RT-WDF: modular wave digital filter library (DAFx)](https://www.dafx.de/paper-archive/details/ilZF4akpmSxzyiusoAOolg)
- [libmksim (Rust, MIT): real-time circuit simulation for audio DSP](https://github.com/mkaudio-company/libmksim)
- Antialiasing: Parker, Zavalishin, Le Bivic, "Reducing the aliasing of
  nonlinear waveshaping using continuous-time convolution" (DAFx-16);
  Bilbao, Esqueda, Parker, Välimäki, "Antiderivative antialiasing for
  memoryless nonlinearities" (IEEE SPL 2017); see also
  [the companion code](https://github.com/julian-parker/DAFX-AntiAliasing),
  [antiderivative antialiasing for stateful systems](https://www.hsu-hh.de/ant/wp-content/uploads/sites/699/2020/10/DAFx2019_paper_4.pdf),
  and [practical considerations for ADAA](https://jatinchowdhury18.medium.com/practical-considerations-for-antiderivative-anti-aliasing-d5847167f510)
- Transformers: [a transformer model based on the Jiles-Atherton theory](https://www.researchgate.net/publication/270465159_A_transformer_model_based_on_the_Jiles-Atherton_theory_of_ferromagnetic_hysteresis),
  [real-time audio transformer emulation for virtual tube amplifiers](https://www.researchgate.net/publication/220057543_Real-Time_Audio_Transformer_Emulation_for_Virtual_Tube_Amplifiers)
- [Deep learning for tube amplifier emulation](https://arxiv.org/pdf/1811.00334)

---

## 10. Phase 0 findings

A throw-away prototype lives in [research/analog-spike/](research/analog-spike/)
(a standalone Rust program, not part of the player; run it with
`cargo run --release`, output saved in
[RESULTS.txt](research/analog-spike/RESULTS.txt)). It is unoptimized research
code, so treat the numbers as ballpark, measured on an Intel Core i9-9900K.

### 10.1 What the prototype has

- Four memoryless shapers: an asymmetric tanh (a bias term adds even
  harmonics), a symmetric tanh, a hard clip, and a **12AX7 triode stage
  computed from Koren's equations** (300 V supply, 100 kΩ load, about −1.5 V
  bias) turned into a lookup table.
- Anti-aliasing options: none, first-order ADAA (analytic antiderivative),
  and 2x, 4x and 8x oversampling (Kaiser-windowed FIR, 32 taps per phase).
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

- The **triode stage behaves like the textbook**: the 2nd harmonic dominates
  (about 20 dB above the 3rd), and it rises 1 dB per dB of input, while the
  3rd rises 2 dB per dB. That is the "warm" signature and it falls out of
  Koren's equations with no tuning.
- A **symmetric curve gives only odd harmonics** (no 2nd or 4th), as
  expected for push-pull and transistor stages.
- A **hard clip is perfectly clean until it clips**, then produces strong odd
  harmonics. That is the "hard transistor" flavour, and it is why a soft
  knee is much more pleasant at moderate levels.
- The asymmetric tanh is a workable, cheaper stand-in for the triode, but
  its harmonics rise faster than the real curve's. Its 4th harmonic is much
  higher than the triode's.

These are **provisional targets** (item 0.2): they come from my model, not
from published measurements of real tubes.

### 10.3 Aliasing (the risk that decides the design)

Level of everything in the audible band that is *not* a harmonic of the
tone (that is aliasing), relative to the tone. Heavy test: asymmetric tanh
at gain 3 and input 0.8, deliberately harsh.

| Sample rate, tone | No protection | ADAA (1st order) | 2x oversampling | 4x | 8x |
|---|---|---|---|---|---|
| 44.1 kHz, 6 kHz | −26 dB | −38 dB | −63 dB | −106 dB | −105 dB |
| 44.1 kHz, 9.5 kHz | −14 dB | −26 dB | −42 dB | −91 dB | −104 dB |
| 96 kHz, 9.5 kHz | −51 dB | −75 dB | −101 dB | −133 dB | −130 dB |

- At 44.1 kHz, unprotected aliasing is **very audible** (as loud as −14 to
  −26 dB relative to the tone). It has to be handled.
- **4x oversampling is clean** at 44.1 and 48 kHz (better than −90 dB).
  2x is adequate for gentle drive but not for a hard test tone.
- **First-order ADAA alone is not enough at 44.1 kHz** for heavy drive
  (−26 to −38 dB), but it is good at 96 kHz and above (−75 dB). ADAA
  combined with 2x oversampling is the standard compromise.
- At 96 kHz and above, **2x is plenty**, and unprotected is already
  −51 dB for this harsh case.

A correction to my own first run: an early version of this test reported
about −29 dB for every oversampling factor. That was a bug in the test
(the filter's end-of-buffer edge and passband droop were being counted as
aliasing), not a property of oversampling. The numbers above are after the
fix.

### 10.4 CPU (one second of stereo audio, one core, this machine)

| Sample rate | Existing 8-band EQ | Naive shaper | Triode table | ADAA1 | 2x OS | 4x OS |
|---|---|---|---|---|---|---|
| 44.1 kHz | 0.2% | 0.1% | 0.1% | 0.2% | 1.1% | 2.1% |
| 96 kHz | 0.4% | 0.2% | 0.1% | 0.5% | 2.3% | 4.8% |
| 192 kHz | 0.8% | 0.5% | 0.2% | 1.1% | 4.7% | 9.8% |

- The shaper itself is nearly free; **oversampling is the cost**, and even a
  crude implementation of it (a direct FIR in f64) fits comfortably.
- **CPU is not the constraint** for this feature. A 4x oversampled stage at
  192 kHz is about 10% of a core in unoptimized code. A polyphase
  half-band or f32 implementation should be several times cheaper
  (**[guess]**).
- Practical rule that follows: **4x at 44.1 and 48 kHz, 2x at 88.2 and 96
  kHz, ADAA-only or none at 176.4 kHz and above.**

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
- Whether the table-based triode sounds right by ear; the prototype only
  measures it.
- Transformer and sag models (Phase 3) were researched but not prototyped.
- A quick check of the prototype's triode curve against a datasheet
  (the Koren model gave about 0.94 mA at a grid bias of −2 V and 250 V plate
  for a 12AX7, which is plausible but not verified).

---

## 11. Phase 1 results

Code: [analog.rs](crates/kahawai-player-core/src/analog.rs) (the stage),
the `DspStage` trait in [dsp.rs](crates/kahawai-player-core/src/dsp.rs),
and the wiring in [engine.rs](crates/kahawai-player-core/src/engine.rs).
Tauri command: `set_analog`. **There is no UI yet (that is Phase 4).**

### 11.1 What exists

- **`DspStage` trait** (`prepare(sample_rate)`, `process(interleaved, channels)`,
  `latency_frames()`, `reset()`), implemented by both the EQ and the analog
  stage. The plan's sketch had `prepare(rate, channels)` and a `process`
  without a channel count; the built version follows the EQ's existing
  `process(samples, channels)` shape so nothing else had to change.
- **`AnalogStage`**, with these settings (all clamped, saved in
  `engine-settings.json` under `dsp.analog`, off by default; older settings
  files load fine):

| Setting | Meaning | Range / default |
|---|---|---|
| `enabled` | Stage on or off | off |
| `flavour` | `warm_triode` (asymmetric, even harmonics) or `solid_state` (symmetric, odd harmonics) | warm_triode |
| `drive` | How hard the signal is pushed into the curve | 0 to 1, default 0.4 |
| `mix` | Parallel blend of the processed signal | 0 to 1, default 0.4 |
| `output_db` | Output trim | plus or minus 6 dB, default 0 |
| `auto_gain` | Match the processed level to the dry level (at a −12 dBFS RMS reference sine) | on |

- **Oversampling by sample rate:** 4x up to 50 kHz, 2x up to 100 kHz, none
  above (`oversample_factor`), as recommended in section 10.5.
- **Latency:** while oversampling is active (up to 100 kHz) the FIR delays
  the signal 32 frames: 0.73 ms at 44.1 kHz, 0.67 ms at 48 kHz, 0.33 ms at
  96 kHz; none above 100 kHz. The dry path is delayed by the same amount so
  the mix is time-aligned. The delay is **not** yet compensated in the
  playhead (under a millisecond); `latency_frames()` reports it for later.
- **Live changes:** drive, mix, output and gain match glide over about 15 ms;
  on/off cross-fades; a flavour change fades out, swaps the curve, and fades
  back in, so nothing clicks. Fully off, the stage is bit-transparent.
- **Bypass rules:** it runs only on the shared PCM path, after the EQ and
  before loudness and volume. DoP and bit-perfect never call it (the two
  existing bit-identical tests now switch the stage on, hostile settings
  included, and still pass).

### 11.2 How the curves behave

- **Warm triode** is an asymmetric tanh, `tanh(g x + bias) − tanh(bias)`,
  scaled to unity small-signal gain, with the bias growing with the square
  root of drive. At zero drive it is linear and clean; even a little drive is
  lopsided. A DC blocker (10 Hz) removes the offset the asymmetry creates.
  At moderate drive the 2nd harmonic sits about 24 dB above the 3rd
  (drive 0.4, input level 0.2: 2nd −21 dB, 3rd −45 dB relative to the tone).
- **Solid state** is a symmetric `tanh(g x) / g`: no even harmonics (better
  than −70 dB in the test), a soft odd-harmonic edge that grows with level.
- **Warm triode is still an approximation.** A first attempt with a fixed
  bias of 0.4 lost the even-harmonic dominance as soon as it was driven
  (2nd about equal to 3rd), which is why the bias now scales with drive.
  Phase 2 replaces this curve with the one derived from the Koren tube
  model.

### 11.3 Tests (all pass)

The stage's tests are in `analog.rs`, plus two engine tests:

- bit-transparent when off, and again after fading out;
- warm triode: 2nd harmonic well above the 3rd; solid state: no 2nd, audible
  3rd;
- harmonics grow with level and drive, and a quiet signal at zero drive stays
  clean;
- aliasing stays below the audible threshold: better than −60 dB at 44.1 and
  48 kHz and better than −75 dB at 96 kHz, on the hard test tone that aliased
  at −14 dB without protection;
- dry and wet are time-aligned (a linear-regime tone comes out as the input
  delayed by exactly the reported latency);
- auto gain keeps the processed level within 1 dB of the dry level, for both
  flavours at low and full drive;
- live parameter changes and flavour changes do not click (the tests fail if
  the fade is removed; I checked by shortening it);
- settings are clamped, and missing fields take defaults;
- engine: the stage colours PCM output, is off by default, and persists;
  DoP and bit-perfect bytes are identical with it on.

### 11.4 CPU (real stage, release build, one second of stereo audio)

| Sample rate | Existing 8-band EQ | Warm triode | Solid state |
|---|---|---|---|
| 44.1 kHz (4x) | 0.2% | 3.4% | 3.3% |
| 96 kHz (2x) | 0.4% | 3.8% | 3.8% |
| 192 kHz (none) | 0.9% | 1.0% | 0.9% |

Measured by [phase1_cost.rs](research/analog-spike/src/bin/phase1_cost.rs)
(output in [PHASE1_COST.txt](research/analog-spike/PHASE1_COST.txt)).
Plenty of room. The FIR is a straightforward implementation; there is
obvious headroom for optimisation if it is ever needed.

### 11.5 How to try it now (no UI yet)

Quit the app, edit `engine-settings.json` in the app's data folder
(`~/Library/Application Support/com.suayan.kahawai-player/` on macOS), and
add this inside the `"dsp"` object:

```json
"analog": { "enabled": true, "flavour": "warm_triode", "drive": 0.5, "mix": 0.5,
            "output_db": 0.0, "auto_gain": true }
```

Start the app and play something on the normal (shared) output. Use
`"solid_state"` for the other flavour. It will not apply on DoP or
bit-perfect output.

### 11.6 Known limits and what is next

- No UI, so no live A/B in the app yet (Phase 4).
- The warm-triode curve is a placeholder for the Koren-derived one (Phase 2).
- No ADAA yet (Phase 2), and no sag or transformer colour (Phase 3).
- No clip protection: with the output trim up or hot material and high
  drive, the result can exceed 0 dBFS. The gain-match reference is a
  −12 dBFS RMS sine, so loud music will not be perfectly level-matched.
- Switching the stage on from bypass shifts the un-delayed input to a
  32-frame delayed path during the 15 ms fade; that is inaudible in
  practice but is a small comb-filter moment.
