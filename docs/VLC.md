# Playing your library in VLC

Kahawai Player is the best way to hear your library, but it isn't the only one. Any computer on your home network with [VLC](https://www.videolan.org/vlc/) can play a Kahawai playlist or album: titles and artists in VLC's playlist, artwork, one track after another, and seeking. VLC needs no login, because the Kahawai Server has none; it is meant for a trusted home network only.

## Open a playlist or album

Each playlist and album has an address that hands VLC a ready-made playlist:

```
http://<server>:8080/api/playlists/<id>/export?format=m3u
http://<server>:8080/api/albums/<id>/export?format=m3u
```

`<server>` is the address the Player connects to (**Settings → Server** in the Player shows it). In VLC choose **Media → Open Network Stream…** (on a Mac, **File → Open Network…**), paste the address, and press **Play**. The whole playlist loads and plays in order.

To find an `<id>`, open these in a web browser on the same network:

- **Playlists:** `http://<server>:8080/api/playlists` lists every playlist with its `id` and `name`.
- **Albums:** search for a track on it, as in `http://<server>:8080/api/search?q=waltz for debby`. Each result carries its album's `album_id`.

Two other formats are available by changing the end of the address: `format=xspf` adds cover art to VLC's playlist, and `format=pls` suits older players. Plain M3U is the safest choice for VLC.

You can also save the playlist file and open it later. It holds full addresses, so it keeps working as long as the server is running at the same address.

## What plays, and how

- **MP3, FLAC, AAC/M4A, WAV, AIFF, Ogg Vorbis and Opus** reach VLC exactly as they are on disk. Seeking is instant.
- **DSD (DSF and DFF)** is converted to 24-bit FLAC at 88.2 kHz, because VLC can't take DSD directly. Converting is slow (on our test Mac, a five-minute track takes a little over three minutes), so the first time you play a DSD track it starts right away but can't be seeked and shows no length. The server keeps the converted file, and from the next play on, seeking and the track length work like any other file. The server keeps up to 8 GB of converted tracks (`transcode_cache_mb` in its settings file, in MB; `0` turns this off), and lets the least recently played go first.
- **SACD ISO images** aren't included: the server can't play them. Neither are tracks whose files have gone missing. The playlist simply skips them.

## What VLC can't do here

VLC can't browse the library itself. Opening the server's plain address shows nothing useful; you open one playlist or album at a time. VLC can't see the Player's queue either, which lives in the Player. To take a queue to VLC, save it as a playlist in the Player first, then open that playlist.

## Another route: the music share itself

If your music lives on a NAS, VLC can open the shared folder directly, with no Kahawai involved: **Media → Open Network Stream…** and an address like `smb://nas.local/music/`, or VLC's **Local Network** browser. Seeking works and VLC asks for the share's user name and password (the NAS must allow SMB2 or later). You get folders and files rather than Kahawai's albums and playlists, and DSD files arrive unconverted, so whether they play is up to your copy of VLC.
