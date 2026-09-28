# SACD extraction — not planned

Earlier docs in this repo (`kahawai-server-spec.md`, `DESIGN.md`, `docs/Roadmap.md`, the two READMEs) described SACD ISO support as a staged feature: cataloged in v1, decoded via an external `sacd_extract` step "soon," with in-server ISO/DST decoding pushed to v2. That framing is retired. **There is no intent to implement SACD extraction, in any form, at any point.**

## What Kahawai actually does with a `.iso` file today

- The scanner recognizes it and adds a catalog row (title/artist metadata where readable) but marks it `decodable: false` / `streamable: false`.
- `GET /stream/:id` (and any `?format=`) on that row returns `415`.
- The server never shells out to any external tool. It never will.

There is no `extract_iso` worker, no bundled or expected `sacd_extract` binary, and no roadmap item to add one. Any prior job-queue plumbing that still references an ISO-extraction job kind is dead code kept only because removing it isn't worth the churn — it is not a sign of a feature in progress.

## Why

- **DST decompression** (the compression SACD uses internally, especially multichannel) is a substantial, narrow decoding project on its own — disproportionate to how many people in Kahawai's actual audience own SACD media.
- **Ripping an SACD to an ISO in the first place needs specific hardware** (a PS3 with pre-3.55 firmware and a homebrew ripper, or a hacked standalone player) — a legal and technical dependency entirely outside anything a music server can help with. Kahawai isn't going to build a UI around a step most users can't take anyway.
- Kahawai's actual hi-res story — native DSD (DSF/DFF) with DoP hog-mode output — already covers the format family this project cares about without touching SACD's disc-level copy protection at all.

## What to do if you have SACD ISOs

Extract them yourself, outside Kahawai, with the community [`sacd_extract`](https://github.com/sacd-ripper/sacd-extract) tool (or whatever tool you already use). Drop the resulting `.dsf`/`.dff` files into your music folder like any other file — Kahawai scans, catalogs, and streams those natively, including native DoP playback in the Player. The ISO itself can stay wherever you keep it; Kahawai has nothing to do with that half of the workflow, on purpose.
