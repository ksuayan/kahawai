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
- **Audiobooks:** a separate library of books, as a grid or a list, with a Continue
  listening shelf. A book picks up where you left off, with its chapters, bookmarks,
  listening history by day, and its own speed (0.75× to 2.5×, without changing the
  voice's pitch) and skip times. A sleep timer fades out after a set time or at the end
  of the chapter. Playing a book puts your music queue aside and gives it back exactly
  when you return. Several listeners can share one server, each with their own place.
- **Podcasts:** follow shows from your Kahawai server, with Up Next, the episodes you're
  part-way through, show notes, and each show's own speed, skip times and download rules.
  Episodes pick up where you left off and play whether or not the server has downloaded
  them. Playing one puts your music aside, as a book does.
- **Internet radio:** play your favorite stations, search the station directory (when the
  server's Online sources switch is on), or add a station by its stream address, with the
  song that's on and a reconnect if the stream drops.
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

- **The player talks to your Kahawai server,** and to nothing else on its own. The
  exceptions are things you ask for: a radio station you play is streamed straight from
  that station, station logos and podcast artwork load from where they're published, and
  links in show notes open in your browser. It has no accounts and no analytics.
- **The server has no authentication and no encryption.** Run it on a private network you
  trust; the player warns you when it cannot reach it.
- **Your settings stay on this computer.** The server address, output device, equalizer,
  queue and preferences are stored locally. Where you are in each audiobook, your
  bookmarks and listening history are kept on your Kahawai server, for each listener,
  so any player in the house picks up where you left off.

## Copyright and trademarks

Kahawai Player is Copyright (c) 2026 Kyo Suayan, and licensed under the GNU Affero
General Public License, version 3 or later (see License below).

MQA is a trademark of MQA Limited. Apple, macOS and Core Audio are trademarks of Apple
Inc. Tube and transistor type numbers (12AX7, 300B, EL34 and the like) are used only to
describe which part a modelled stage is based on. Kahawai Player is an independent
product and is not affiliated with or endorsed by any of these companies or by any
maker of the equipment it emulates.

## License

Kahawai Player is free software: you can redistribute it and/or modify it under the terms of the
GNU Affero General Public License as published by the Free Software Foundation, either
version 3 of the License, or (at your option) any later version.

It is distributed in the hope that it will be useful, but **without any warranty**; without
even the implied warranty of merchantability or fitness for a particular purpose. The full
text of the GNU Affero General Public License is in the License tab above, and in the
`LICENSE` file with the source code.

**Source code:** [github.com/ksuayan/kahawai](https://github.com/ksuayan/kahawai).

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
