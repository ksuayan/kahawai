# Backlog

Things we decided to leave for later, with enough context to pick them up
cold. Newest thinking wins: edit freely. Size is a rough guess:
**S** = an hour or two, **M** = a day, **L** = several days.

## EQ and audio

| Item | Size | Why / notes |
|---|---|---|
| **Smooth by parameter, not by coefficient** | M | Live EQ edits currently glide the filter *coefficients* linearly over ~15 ms ([dsp.rs](crates/kahawai-player-core/src/dsp.rs)). That is click-free for the ranges the UI allows, but the response midway is not exactly what the parameters imply, so a fast, large sweep of a narrow, high-gain band can overshoot briefly. The exact way is to interpolate frequency (log), gain (dB) and Q and redesign the filter every few samples: more CPU, more code. Do it only if artifacts are heard on extreme edits. (Unrelated to the graph's axes, which are already log frequency and dB.) See EQ.md, limitation 4.2. |
| **Headroom: auto pre-gain or limiter** | M | There is none. Boosting bands, plus loudness normalization's gain of up to +12 dB, can push the signal past 0 dBFS and clip at the device. Today the dialog only warns (and colours the points). An automatic pre-gain equal to the peak boost, or a soft limiter, would fix it. Decide whether it is on by default. |
| **Check rate changes inside a gapless or chained response** | S | The EQ is designed when a track opens. If a single chained response can switch sample rate without the engine reopening the stream, the EQ would run at the wrong rate for the second track. Unverified: find out whether it can happen, then either add a redesign at the boundary or note that it cannot. |
| **Audio Unit hosting (macOS)** | L | Explored in EQ.md section 6: a `DspStage` trait, an AU stage behind `cfg(target_os = "macos")`, a generic parameter list first, then state persistence. Open questions there (better EQ vs plug-in hosting, latency policy, entitlements). Start with a spike using Apple's `AUNBandEQ`. |
| **Per-channel or mid/side EQ** | M | Every channel gets the same filters. Useful for headphones and for correcting one speaker. |
| **Move user EQ presets out of the webview** | S | User presets live in the webview's localStorage: per machine, lost if app data is cleared. Storing them in `engine-settings.json` (or its own file) next to the engine settings would make them durable and let the engine own them. |
| **Tune the built-in presets and severity thresholds by ear** | S | The six presets (Flat, Classical, Jazz, Rock, Pop, Talk Show) and the yellow/red thresholds (`SEVERITY` in [eqResponse.ts](player/ui/src/eqResponse.ts)) are educated guesses, kept gentle on purpose. |
| **Graph shows the phase or group delay, or an analyzer** | L | Optional polish: a real-time spectrum behind the curve would make it clear what a band is doing to the music. Needs the engine to expose FFT data, so it is a bigger step. |

## Playback and streaming

| Item | Size | Why / notes |
|---|---|---|
| **Real read-ahead buffer** | M | The stream is progressive HTTP with no read-ahead, so the "received" fill on the seek bar sits only a few seconds ahead of the playhead. A background thread that fills 10 to 30 s would make that bar meaningful and would protect against Wi-Fi dropouts. It interacts with seeking (a seek past the buffer restarts the request). Do it only if stutters show up. |
| **Buffered fill for transcoded streams** | M | Transcoded (chunked) responses have no known size, so no fill is drawn. Would need the server to send an estimated length, or to compute progress by duration. |
| **Save the playhead on quit, not only every 5 s** | S | The queue and playhead are saved on pause and every 5 s while playing; a hard quit can lose up to 5 s of position. A shutdown hook in the shell would close the gap. |
| **Reorder and remove in the queue without restarting the track** | M | The core has no in-place reorder or remove command, so the UI re-sends the whole queue, which restarts the current track and re-seeks as an approximation. Add real `queue_move` and `queue_remove` commands. |
| **Restore "Play next" on the two-button layouts** | S | The Now Playing screen and the album page replaced the ⋯ menu with "Add to queue" and "Add to playlist…", which dropped "Play next" (still on track rows). Add it back if it is missed. |
| **MQA is detect-only** | n/a | There is no software MQA decode (proprietary). The bit-perfect path lets an MQA DAC decode it. Nothing to do unless the licensing picture changes. |

## Look and feel

Branch: `look-and-feel` (see [guidelines/Visual-House-Style-Guide.md](guidelines/Visual-House-Style-Guide.md)).

| Item | Size | Why / notes |
|---|---|---|
| **Review both themes by eye** | S | Light and dark were built and tested in code, not looked at on screen. Walk every view in both. |
| **Serif prose size in Settings** | S | The guide says 16 px / 1.6 for prose, applied to all the explanatory paragraphs in Settings. It may be too big for that many one-liners. One `prose-text` class in [style.css](player/ui/src/style.css) controls it. |
| **Muted text contrast in light mode** | S | The guide's `--text-3` (`#8e8e96`) on `#fafafa` is about 3.3:1, below the 4.5:1 the guide itself asks for. Darken it for small captions. |
| **Use the micro-label style** | S | The utility exists but nothing uses it yet (the app has no table headers or eyebrow labels). Apply it when such labels appear, for example queue column headers. |
| **Plex Mono for IDs and technical readouts** | S | Not bundled. Candidates: the audio-chain badge, hashes, sample rates. Adds a font download. |
| **Decide whether `guidelines/` goes in the repo** | S | The folder is untracked. The tests and code now refer to it. |

## DSP effects (research first)

Both items below need the same groundwork, so plan them together:

- **A stage seam.** Extract the `DspStage` trait sketched in EQ.md
  section 6, so the EQ, tape and tube stages are interchangeable and
  orderable in the PCM chain (decode, resample, [stages], loudness,
  volume).
- **Oversampling.** Saturation creates harmonics above the original
  bandwidth; without 2x to 8x oversampling around the non-linear part they
  alias back as harsh, inharmonic distortion. This is the main CPU cost,
  and the playback thread must still keep ahead of the device (see the
  200 ms ring buffer in [cpal_sink.rs](crates/kahawai-player-audio/src/cpal_sink.rs)).
- **Same path rules as the EQ.** PCM shared path only. DoP and bit-perfect
  playback bypass these effects, and the UI dims them with the same
  "not supported for this stream type" message.
- **Level discipline.** Non-linear stages care about input level. Decide
  where in the chain they sit relative to loudness normalization and volume
  (level in, level out, and a drive control), and add a gain-matched A/B so
  "better" is not just "louder".
- **Live edits without clicks.** Reuse the EQ's fade approach for parameter
  changes and on/off.
- **Licensing and naming.** Prefer our own implementations from published
  papers and measurements. Check the licence before borrowing from open-source
  emulations (some are GPL). Use "inspired by" wording for classic gear names;
  the names are other companies' trademarks.

| Item | Size | Why / notes |
|---|---|---|
| **Analog tape emulation, with classic machines as presets** | L (research: M) | A chain of models, not one effect: input saturation with magnetic hysteresis (Jiles-Atherton or a simpler bias-curve model), head bump and gap loss (low-frequency bump, high-frequency roll-off that depends on tape speed), wow and flutter (slow and fast pitch modulation), compression from tape saturation, noise floor and hiss (optional, off by default), record and playback EQ curves (NAB / IEC), and optionally azimuth error and crosstalk. Presets would set speed, tape type and the parameters above for a few well-documented machines and formats (for example a studio 2-inch machine, a consumer reel-to-reel, a cassette deck with and without Dolby-style noise reduction). Research needed: pick the reference machines, find measurements (published frequency responses, THD versus level, wow and flutter figures), decide model depth versus CPU, and listen-test against real recordings. |
| **Tube and transistor "euphonics", with a few popular models as presets** | L (research: M) | Model the character, not a circuit-level simulation: static waveshaping with asymmetry (even-order harmonics for triode-style stages, odd-order for push-pull and transistor clipping), a soft knee that reacts to level, power-supply sag and recovery, output-transformer low-frequency saturation and high-frequency roll-off, and a bias control. Presets would be a handful of well-known stages, described as flavours: a warm single-ended tube, a push-pull tube amp, a solid-state console preamp, a hard-clipping transistor stage. Research needed: which models to include, measured harmonic profiles (2nd/3rd/5th harmonic levels versus drive), how much dynamics (sag) is worth the CPU, and whether to chain with a tone-stack style EQ. A drive control plus a mix (parallel) control keeps it subtle by default. |
| **Effects chain UI** | M | A single place to enable, order and preset the effects (EQ, tape, tube), in the same modal style as the EQ dialog, each with the level meters and the "not supported for this stream type" dimming. Needs the stage seam above and a per-stage on/off with fades. |

## Housekeeping

| Item | Size | Why / notes |
|---|---|---|
| **Delete merged branches** | S | `reka-poc` and `fix-album-grouping` are merged into `main`. Delete with `git branch -d`. |
| **Server tests that fail on this Mac** | S | 10 server tests (`integration_tests::*`, `scan_tests::*`) fail here only because the local ffmpeg lacks `libvorbis`. They pass elsewhere. Skip them when the encoder is missing, so a red run always means something. |
| **Verify queue restore in the real app** | S | The fix and its tests are in, but a full quit-and-relaunch has not been done by hand yet. |
