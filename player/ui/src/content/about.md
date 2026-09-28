# Kahawai Player

Version {{version}}

A desktop music player for your own Kahawai server: it plays your library over your
home network, with careful audio output for people who care how it sounds. The music
stays on your server; nothing about it is sent anywhere else.

## What it does

- **Library:** browse albums, artists and playlists, search, and see each track's format,
  sample rate and bit depth. Album art is cached on disk so browsing stays quick.
- **Playback:** gapless queue with shuffle and repeat, seeking, volume, and a queue that
  survives restarts (with the position you were at).
- **Formats:** FLAC, ALAC, AAC, MP3, Ogg Vorbis, Opus, WAV, AIFF, and DSD (DSF and DFF).
  MQA files are detected and labelled.
- **Audio output:** choose the output device. On macOS the player can take exclusive
  control of a DAC and send it the file's samples untouched (**bit-perfect**), or send
  DSD as **DoP**, so a DAC that decodes MQA or DSD sees the real signal.
- **Sound shaping:** an 8-band parametric equalizer with presets and a live response
  graph, loudness normalization, and an optional **analog warmth** stage that adds the
  character of tubes and transistors, with an A/B panel, level meter and blind test to
  judge it honestly.
- **Look:** dark and light themes, IBM Plex type.

## How audio reaches your ears

| Output | What the player does to the audio |
|---|---|
| Shared (default) | Decode, resample only if the device needs another rate, then optional EQ, analog warmth, loudness and volume |
| Bit-perfect (macOS, exclusive) | Sends the decoded samples to the device untouched, at the file's own rate: no EQ, warmth, loudness, volume or resampling |
| DoP (macOS, exclusive) | Sends DSD inside PCM frames, untouched, to a DAC that understands DoP |

The equalizer and the analog warmth stage never run on the two exclusive outputs, and the
player says so where you would look for them.

## Your music and your privacy

- **The player only talks to your Kahawai server.** It has no accounts, no analytics and
  no network connection to anything else.
- **The server has no authentication and no encryption.** Run it on a private network you
  trust; the player warns you when it cannot reach it.
- **Your settings stay on this computer.** The server address, output device, equalizer,
  queue and preferences are stored locally.

## Copyright and trademarks

Kahawai Player is Copyright (c) 2026 Kyo Suayan. All rights reserved.

MQA is a trademark of MQA Limited. Apple, macOS and Core Audio are trademarks of Apple
Inc. Tube and transistor type numbers (12AX7, 300B, EL34 and the like) are used only to
describe which part a modelled stage is based on. Kahawai Player is an independent
product and is not affiliated with or endorsed by any of these companies or by any
maker of the equipment it emulates.

## Fonts

The interface uses **IBM Plex Sans**, and longer explanatory text uses **IBM Plex Serif**,
both Copyright 2019 IBM Corp. and licensed under the SIL Open Font License, Version 1.1.
The full license text is in the open-source notices.

## Models and standards

- **Tube models:** the tube stages are computed from Norman Koren's triode and pentode
  equations, using the datasheet-fitted parameters in his SPICE tube library.
- **Equalizer:** filter designs follow Robert Bristow-Johnson's "Audio EQ Cookbook".
- **Loudness:** measurement follows the K-weighting of ITU-R BS.1770 (as used by EBU R128).
- **Anti-aliasing:** the analog stage uses oversampling and first-order antiderivative
  antialiasing (Parker, Zavalishin and Le Bivic; Bilbao, Esqueda, Parker and Välimäki).
- **Design notes:** the reasoning, measurements and sources are in the project's
  `docs/v1/Analog-Emulation.md` and `docs/v1/EQ.md`.

## Open-source software

The player is built with open-source software, including Tauri, Vue, Reka UI, Tailwind
CSS, Symphonia, cpal and Opus. Names, versions and licenses are listed in the open-source
notices, including the components under weak-copyleft licenses.
