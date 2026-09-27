# Backlog

Things we decided to leave for later, with enough context to pick them up
cold. Newest thinking wins: edit freely. Size is a rough guess:
**S** = an hour or two, **M** = a day, **L** = several days.

## EQ and audio

| Item | Size | Why / notes |
|---|---|---|
| **Smooth by parameter, not by coefficient** | M | Live EQ edits currently glide the filter *coefficients* linearly over ~15 ms ([dsp.rs](crates/kahawai-player-core/src/dsp.rs)). That is click-free for the ranges the UI allows, but the response midway is not exactly what the parameters imply, so a fast, large sweep of a narrow, high-gain band can overshoot briefly. The exact way is to interpolate frequency (log), gain (dB) and Q and redesign the filter every few samples: more CPU, more code. Do it only if artifacts are heard on extreme edits. (Unrelated to the graph's axes, which are already log frequency and dB.) See EQ.md, limitation 4.2. |
| **Headroom: a true look-ahead limiter** | M | Partly done. Loudness gain is now planned against the track's real peak and the EQ's worst-case boost, so it cannot clip, and a soft guard ([dsp.rs](crates/kahawai-player-core/src/dsp.rs), `headroom_guard`) bends whatever still exceeds full scale. What is left: with loudness normalization off, a large EQ boost is only caught by that guard, which behaves like a hard clip for big overshoots. A look-ahead limiter (a few ms of delay, gain reduction that starts before the peak) would be transparent. It adds latency, which the sample-exact gapless tests would need to account for. |
| **Check rate changes inside a gapless or chained response** | S | The EQ is designed when a track opens. If a single chained response can switch sample rate without the engine reopening the stream, the EQ would run at the wrong rate for the second track. Unverified: find out whether it can happen, then either add a redesign at the boundary or note that it cannot. |
| **Audio Unit hosting (macOS)** | L | Explored in EQ.md section 6: a `DspStage` trait, an AU stage behind `cfg(target_os = "macos")`, a generic parameter list first, then state persistence. Open questions there (better EQ vs plug-in hosting, latency policy, entitlements). Start with a spike using Apple's `AUNBandEQ`. |
| **Per-channel or mid/side EQ** | M | Every channel gets the same filters. Useful for headphones and for correcting one speaker. |
| **Move user EQ presets out of the webview** | S | User presets live in the webview's localStorage: per machine, lost if app data is cleared. Storing them in `engine-settings.json` (or its own file) next to the engine settings would make them durable and let the engine own them. |
| **Tune the built-in presets and severity thresholds by ear** | S | The six presets (Flat, Classical, Jazz, Rock, Pop, Talk Show) and the yellow/red thresholds (`SEVERITY` in [eqResponse.ts](player/ui/src/eqResponse.ts)) are educated guesses, kept gentle on purpose. |
| **Live frequency spectrum visualization** | L | Optional polish, and bigger than it looks. The engine has no FFT today; it would need to run one on the PCM chunks already flowing through `pump_pcm` ([engine.rs](crates/kahawai-player-core/src/engine.rs)) — smoothed magnitude bins at a UI-friendly rate (~30 Hz), sent to the shell alongside the existing snapshot rather than per-sample. Two uses: behind the EQ curve in the EQ dialog (makes it obvious what a band is doing to the actual music, not just the theoretical response), and/or a standalone analyzer/visualizer view (Now Playing, screensaver-style). DoP and bit-perfect bypass the DSP chain entirely, so there is nothing to analyze on those paths — show it disabled there, same as the EQ. Decide bar/line style and whether it is linear or log-frequency (log matches the EQ graph). |

## Playback and streaming

| Item | Size | Why / notes |
|---|---|---|
| **Real read-ahead buffer** | M | The stream is progressive HTTP with no read-ahead, so the "received" fill on the seek bar sits only a few seconds ahead of the playhead. A background thread that fills 10 to 30 s would make that bar meaningful and would protect against Wi-Fi dropouts. It interacts with seeking (a seek past the buffer restarts the request). Do it only if stutters show up. |
| **Buffered fill for transcoded streams** | M | Transcoded (chunked) responses have no known size, so no fill is drawn. Would need the server to send an estimated length, or to compute progress by duration. |
| **Save the playhead on quit, not only every 5 s** | S | The queue and playhead are saved on pause and every 5 s while playing; a hard quit can lose up to 5 s of position. A shutdown hook in the shell would close the gap. |
| **Restore "Play next" on the two-button layouts** | S | The Now Playing screen and the album page replaced the ⋯ menu with "Add to queue" and "Add to playlist…", which dropped "Play next" (still on track rows). Add it back if it is missed. |
| **MQA is detect-only** | n/a | There is no software MQA decode (proprietary). The bit-perfect path lets an MQA DAC decode it. Nothing to do unless the licensing picture changes. |

## Library

| Item | Size | Why / notes |
|---|---|---|
| **Watch the music folders and re-index in the background** | M | Today the catalog only changes on a manual scan or at startup (`scan_on_startup`). Add filesystem watchers on `music_dirs` so files added, removed or changed are picked up automatically, with no button press. **Where:** the server ([main.rs](crates/kahawai-server/src/main.rs) starts the startup scan; [scanner.rs](crates/kahawai-server/src/scanner.rs) has `run_scan_with_progress`; `trigger_scan` in [api.rs](crates/kahawai-server/src/api.rs) shows how a scan job and its lock are created). **Approach:** the `notify` crate (FSEvents on macOS), and **debounce** for several seconds so copying an album does one scan, not hundreds; a burst of events should schedule a single scan. Reuse the existing scan job and `scan_lock`, so it appears as a background job (toast/progress) and never overlaps a manual scan; if one is running, mark "rescan needed" and run again when it finishes. **Deletes:** a removed file should mark its track missing (the scanner already does this), and album and artist rows that end up empty should disappear. **Gotchas:** a half-copied file must not be indexed (wait for its size to stop changing, or skip files modified in the last few seconds); network and external volumes (`/Volumes/...`) can unmount and remount, so handle the root vanishing without wiping the catalog (treat "root missing" as offline, not as everything deleted); watchers can miss events on network shares, so keep a periodic light rescan as a safety net; the OS watch limit on big libraries. **Config:** an on/off switch (default on) and the debounce interval in `config.toml`. **Client:** the player should notice the library changed (the job finishing) and refresh its album list without a restart. |

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
| **Coverflow browsing for albums, old-school iTunes style** | L | A horizontally-scrolling, perspective-tilted stack of album covers as an alternate view of the library (or of the queue), the big cover centered and flipping through with arrow keys, scroll, drag, or a filmstrip below. Needs [Artwork.vue](player/ui/src/components/Artwork.vue)'s covers at a higher resolution than the grid uses, CSS 3D transforms (`perspective` + `rotateY` per card) or a canvas/WebGL renderer if CSS performance is poor with a large library, and a decision on how far it reaches: a view alongside Albums, or a full replacement for browsing. Reflection under the cover is the classic touch. Virtualize the strip (render only covers near the center) so a large library does not tank scroll performance. |

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
| ~~Tube and transistor "euphonics", with a few popular models as presets~~ | done on `analog-poc` | Built: 21 flavours, sag, transformer colour, level meter, blind test and listening suggestions. See [Analog-Emulation.md](Analog-Emulation.md). Follow-ups are in "Analog warmth: what is left" below. |
| **Effects chain UI** | M | A single place to enable, order and preset the effects (EQ, tape, tube), in the same modal style as the EQ dialog, each with the level meters and the "not supported for this stream type" dimming. Needs the stage seam above and a per-stage on/off with fades. |

## Analog warmth: what is left

Everything here follows from the work on `analog-poc` (see
[Analog-Emulation.md](Analog-Emulation.md)); it is built and tested but has
not been listened to yet.

| Item | Size | Why / notes |
|---|---|---|
| **Listen, then tune** | S | Nothing has been heard yet. Use the listening suggestions (section 18 of the doc) and the blind test, then adjust: the defaults (drive 40%, mix 40%, sag 30%, transformer 30%), each flavour's typical sag and transformer values, and the anti-aliasing plan by rate. |
| **Quick-access warmth control in the transport bar** | S | Phase 4.1 of the plan was not done: the stage lives only in Settings. A small button next to the EQ button (on/off, A/B) would put it one click away. |
| **Blind test: keep the result, check the match per passage** | S | The result disappears when closed, and the level match is checked from earlier readings, not the passage playing during the test. Save results (date, slots, score), and re-measure while a test runs. |
| **Check the tube models against datasheets** | M | Only the EL84, 300B and 2A3 biases were compared with published operating points, and those from memory. Compare the small-signal tubes' plate curves and gain with the datasheets, and find published harmonic measurements to replace the provisional targets. |
| **Model more of the real circuit** | L | Left out on purpose: interelectrode capacitance (high-frequency roll-off), cathode bypass and coupling dynamics, screen current and supply sag, a Jiles-Atherton transformer instead of the bass soft clip, hum and noise, and class-AB bias variations. |
| **Faster oversampling** | M | The FIR is a straightforward loop. A polyphase half-band or SIMD version would cost a fraction of the current 3.5% (44.1 kHz) to 4.5% (96 kHz) of a core. Only worth it if CPU becomes a concern. |
| **Build the tube tables at build time** | S | Each flavour's table is computed on first use (about 20 ms on the playback thread, once). Generating them at build time removes that. |
| **Make the analog tests faster** | S | The analog unit tests take about 35 s in a debug build (FFTs and oversampling). Trim the sample counts, or run them with optimizations. |
| **Reuse the level meter for the EQ** | S | The EQ has no automatic pre-gain or limiter (see the headroom item above). The new `LoudnessMeter` and peak tracking could drive an "EQ boost may clip" reading with real numbers instead of the graph's estimate. |
| **Put the effects in a chain** | M | With the EQ and the analog stage both in the PCM path, the "Effects chain UI" item above is now more useful: enable, order and preset them in one place. |

## Housekeeping

| Item | Size | Why / notes |
|---|---|---|
| **Delete merged branches** | S | `reka-poc`, `fix-album-grouping` and `look-and-feel` are merged into `main`. Delete with `git branch -d`. |
| **Server tests that fail on this Mac** | S | 10 server tests (`integration_tests::*`, `scan_tests::*`) fail here only because the local ffmpeg lacks `libvorbis`. They pass elsewhere. Skip them when the encoder is missing, so a red run always means something. |
| **Verify queue restore in the real app** | S | The fix and its tests are in, but a full quit-and-relaunch has not been done by hand yet. |
