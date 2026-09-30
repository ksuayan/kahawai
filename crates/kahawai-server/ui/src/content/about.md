# Kahawai Server

Version {{version}}

The music server behind Kahawai Player: it catalogs the music on your drives and network
shares and streams it to the players on your home network. Your music stays where it is;
the server reads it, never changes it.

## What it does

- **Library:** scans your music folders (a first scan of a large network share is quick:
  tags first, file fingerprints afterwards in the background), groups tracks into albums
  and artists, and keeps up with changes on each rescan.
- **Formats:** FLAC, ALAC, AAC, MP3, Ogg Vorbis, Opus, WAV, AIFF, and DSD (DSF and DFF).
  MQA files are detected and labelled.
- **Streaming:** sends each file as it is, or converts it on the fly when a player asks
  for it: to FLAC, and to Opus or MP3 in builds that include those encoders. DSD can go
  out as DoP for a DAC that understands it. Tracks play back to back without gaps.
- **Genres:** messy genre tags are mapped to a tidy set of genres for browsing and search,
  without touching the tags themselves.
- **Album info (optional, off by default):** looks up albums that have no MusicBrainz ID
  at MusicBrainz, and adds missing years and covers from the Cover Art Archive. It only
  fills in what's missing, and sends album and artist names off your network, which is why
  it's off until you turn it on.
- **Players stay current:** each player keeps its own copy of the library and asks only
  for what changed, so a restart doesn't reload everything.

## Your music and your privacy

- **Nothing leaves your network** unless you turn on album info lookup, which sends album
  and artist names to MusicBrainz.
- **The server has no authentication and no encryption.** Run it on a private network you
  trust.
- **Your files are never modified.** Tags, covers and folders stay exactly as they are; the
  server keeps what it learns in its own database.

## Copyright and trademarks

Kahawai Server is Copyright (c) 2026 Kyo Suayan, and licensed under the GNU Affero
General Public License, version 3 or later (see License below).

MQA is a trademark of MQA Limited. MusicBrainz is a trademark of the MetaBrainz
Foundation. Apple and macOS are trademarks of Apple Inc. Kahawai Server is an independent
product and is not affiliated with or endorsed by any of these companies.

## License

Kahawai Server is free software: you can redistribute it and/or modify it under the terms of the
GNU Affero General Public License as published by the Free Software Foundation, either
version 3 of the License, or (at your option) any later version.

It is distributed in the hope that it will be useful, but **without any warranty**; without
even the implied warranty of merchantability or fitness for a particular purpose. The full
text of the GNU Affero General Public License is in the License tab above, and in the
`LICENSE` file with the source code.

**Source code:** [github.com/ksuayan/kahawai](https://github.com/ksuayan/kahawai). Because the server is used over a network,
the license gives everyone who uses it that way the right to its source code as well:
every Kahawai server names where it is in its `/api/identity` answer (`source_url`), and
the Player's Settings shows it.

## Fonts

The interface uses **IBM Plex Sans**, and longer explanatory text uses **IBM Plex Serif**,
both Copyright 2019 IBM Corp. and licensed under the SIL Open Font License, Version 1.1.
The full license text is in the open-source notices.

## Data sources

- **MusicBrainz** (musicbrainz.org): release data, used under the MetaBrainz Foundation's
  terms, when album info lookup is on.
- **Cover Art Archive** (coverartarchive.org): album covers, which remain the property of
  their rights holders.

## Open-source software

The server is built with open-source software, including Tauri, Vue, Reka UI, Tailwind
CSS, axum, SQLite (through sqlx), lofty and Symphonia. Names, versions and licenses
are listed in the open-source notices, including the components under weak-copyleft
licenses.
