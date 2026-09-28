# Kahawai: a self-hosted streaming server and high-resolution audio player

**For immediate release — September 27, 2026**

Your music library already exists. It sits on a drive somewhere in the house: terabytes of FLAC, a shelf of DSD rips, CDs you encoded yourself back when that took an afternoon. Kahawai starts from the premise that getting those files to your ears shouldn't require anyone else's cloud, anyone's account, or anyone's monthly fee.

Kahawai is two programs that work as one. Kahawai Server catalogs your library — scanning folders into a local database, serving audio over your home network with HTTP range streaming, transcoding formats on the fly when a player needs it, and keeping long background jobs running quietly. Kahawai Player is the listening end: a desktop client that plays the library back with the kind of care the files deserve. Both are written in Rust and released as free software under the GNU Affero General Public License v3.0 — a copyleft license that keeps the code open even when someone runs it as a network service. The source is published on GitHub.

## Your library, on your network, under your control

The server asks for three things: where your music lives, where to keep its database, and which network address to listen on. Everything else follows. It speaks FLAC, WAV, AIFF, OGG Vorbis, Opus, and DSD in DSF and DFF form. Playlists queue the way you'd expect, imports arrive from m3u files with an honest report of what matched and what didn't, and gapless albums play gaplessly — the transitions preserved sample for sample.

One deliberate boundary: this is a trusted-LAN server. There is no login, no encryption, no rate limiting in v1, and it should never face the internet. That isn't an oversight; it's the trade that keeps the design small and auditable. Authentication and TLS are planned for a later version.

## Bit-perfect when it matters, honest about the rest

The player offers two ways to listen, and it tells you which one you're hearing. In shared mode it plays through the operating system's mixer with EQ, loudness compensation, and an Analog Warmth stage — a tube-style saturation circuit you can toggle against the dry signal to hear exactly what it does. In exclusive mode it takes over the audio device at the file's own sample rate and hands the samples across untouched: no EQ, no volume processing, no resampling. That's the path DSD takes as native DSD-over-PCM, and it's the path MQA files take so a decoding DAC receives the stream intact. The player detects MQA at scan time and says so on screen; on equipment without a decoder, the same file simply plays as full CD-quality PCM, because that compatibility is built into the format itself.

Nothing here pretends. The signal-path panel labels what the engine is actually doing, the settings say what each choice costs, and the documentation keeps a public list of what v1 doesn't do yet.

## Status and availability

Kahawai v1 is code-complete, with the full test suite green and the macOS desktop client — built on Tauri 2 and Vue — awaiting final validation on real hardware: first packaged build, CoreAudio behavior on a physical DAC, and signing. The server runs headless on Linux and Windows and ships with a setup interface on macOS.

### About Kahawai

Kahawai is a personal project by Kyo Suayan: a self-hosted music streaming server and high-resolution audio player, written in Rust, for people who own their music libraries and want them treated accordingly. It is free software, licensed under the GNU Affero General Public License v3.0 or later.

Source and documentation: https://github.com/ksuayan/kahawai
