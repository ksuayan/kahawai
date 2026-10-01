# Installing Kahawai (beta)

Thanks for trying Kahawai! This guide covers installing it, what to do when something
doesn't work, and common questions. It's a beta: you'll probably find rough edges, and
hearing about them is the point.

Kahawai has two apps:

- **Kahawai Server** looks after your music library. It runs on the Mac that has your
  music (or can reach it on a network drive), and it needs to keep running while you
  listen.
- **Kahawai Player** is the app you listen with. It can run on the same Mac as the server,
  or on any other Mac on your home network.

## Installation

### What you need

- A Mac with **macOS 11 (Big Sur) or later**, Intel or Apple Silicon.
- Your music as files: FLAC, ALAC, AAC (M4A), MP3, Ogg Vorbis, Opus, WAV, AIFF, or DSD
  (DSF, DFF). Music from streaming services or with copy protection can't be played.
- A home network, if the Player and the Server are on different Macs. Both Macs must be
  on the same network.

### 1. Copy the apps

1. Open **Kahawai.dmg**.
2. Drag **Kahawai Server** and **Kahawai Player** onto the **Applications** folder.
3. Eject the disk image.

### 2. Open them the first time

The beta isn't signed with an Apple developer certificate yet, so macOS asks you to
confirm, once for each app:

- **macOS 15 (Sequoia) or later:** open the app. When macOS says it can't verify it, click
  **Done**. Then open **System Settings → Privacy & Security**, scroll down to the message
  about Kahawai Server, click **Open Anyway**, and confirm with your password or Touch ID.
  Repeat for Kahawai Player.
- **macOS 14 (Sonoma) or earlier:** Control-click (or right-click) the app in Applications,
  choose **Open**, then click **Open** again.

After that, both apps open normally. If macOS says an app **"is damaged and can't be
opened"**, see [Troubleshooting](#an-app-is-damaged-or-wont-open).

### 3. Set up the server

Open **Kahawai Server**. A short setup guides you:

1. **Music folders:** click **Add folder…** and choose the folders with your music (a
   network drive is fine: connect to it in Finder first). Each folder shows how many music
   files it found.
2. **Database:** choose where the server keeps its library database (a few hundred MB for
   a large library). Your own Mac's drive is best.
3. **Review:** check your choices. Leave the **Bind address** as `0.0.0.0:8080` unless you
   know you need something else.
4. Start the server. The first scan begins right away; the **Status** tab shows its
   progress, and a large library can take a while (the tags come first, so albums appear
   early).

macOS may ask:

- to allow Kahawai Server **access to your Music folder**, a **network volume** or a
  **removable volume**: click **Allow**, or it can't read your music;
- whether Kahawai Server may **accept incoming network connections**: click **Allow**, or
  the Player can't reach it.

**Keep the server running while you listen.** Closing its window keeps the server running
(click its Dock icon to bring the window back). **Quit** (⌘Q, or **Quit App** on the Status
tab) stops it. To start it automatically, add it in **System Settings → General → Login
Items**. Quitting the server also lets connected players know it's gone.

### 4. Connect the player

Open **Kahawai Player**, then **Settings** (the gear at the bottom left) → **Server**:

- **Same Mac as the server:** the address is `http://localhost:8080`, already filled in.
- **Another Mac:** enter `http://<server Mac's name>.local:8080`. The name is in **System
  Settings → General → Sharing → Local hostname** on the server's Mac, for example
  `http://Studio-Mac.local:8080`.

Click **Save**. The status line should say **Server reachable · Kahawai Server …**.

On macOS 15, the Player asks to **find devices on your local network**: click **Allow**, or
it can't reach a server on another Mac.

The first time, the Player copies the library over (a few seconds for a large library).
After that it opens straight away.

### Updating

Drag the new versions onto Applications and replace the old ones. Your settings, library
and playlists are kept. You may need to confirm the first opening again (step 2).

### Uninstalling

Delete both apps from Applications. To remove everything they stored as well, delete:

- `~/Library/Application Support/Kahawai Server/` (the server's settings)
- the server's database folder you chose during setup (`music.db`)
- `~/Library/Application Support/com.suayan.kahawai-player/` (the player's settings, queue
  and library copy)
- `~/Library/Caches/com.suayan.kahawai-player/` (album art cache)
- `~/Library/Logs/Kahawai Server/` and `~/Library/Logs/Kahawai Player/` (logs)

(In Finder, **Go → Go to Folder…** and paste the path.) Your music files are never touched.

## Troubleshooting

### An app is damaged or won't open

This is macOS being careful with an unsigned app downloaded from the internet. Open
**Terminal** and run (one line per app):

```
xattr -dr com.apple.quarantine "/Applications/Kahawai Server.app"
xattr -dr com.apple.quarantine "/Applications/Kahawai Player.app"
```

Then open the app again. If it still won't open, please send the log (see
[Sending a log](#sending-a-log)) and your Mac model and macOS version.

### The server says "Server not running"

The Status tab says why:

- **"Another Kahawai Server is running on …"**: another copy of the server has the same
  port. The panel shows which one; click **Stop it and start this server**, or quit the
  other copy.
- **"Another program is already using 0.0.0.0:8080"**: something else uses port 8080. On
  the **Settings** tab, **Change bind/database** lets you pick another port (for example
  `0.0.0.0:8090`); use the same port in the Player's server address.
- Otherwise, click **Restart Server**, and if it keeps failing, send the log.

### The player says "Server unreachable"

1. Check the server is running (its Status tab says **Server running**).
2. Check the address in the Player's **Settings → Server** (name and port).
3. Player on another Mac: both Macs on the same network; in **System Settings → Privacy &
   Security → Local Network**, turn **Kahawai Player** on.
4. On the server's Mac, if the firewall is on: **System Settings → Network → Firewall →
   Options**, and make sure Kahawai Server is allowed.

If Settings says **"Something answers at this address, but it isn't a Kahawai server"**,
the port belongs to another program: check the port number.

The Player keeps working without the server, from its saved copy of the library (a banner
says so), but it needs the server to play.

### Albums are missing

- Make sure the folders are listed in the server's **Settings → Music folders**, and that a
  network drive was connected when the server scanned. To scan again, use the Player's
  **Settings → Library → Scan library** (adding or removing a folder on the server, then
  **Apply**, scans too).
- If macOS asked about folder access and you clicked **Don't Allow**, turn it back on in
  **System Settings → Privacy & Security → Files and Folders** (or **Full Disk Access**).

### Some tracks appear twice

The same music is in two of your folders (a folder and a copy of it, say). Identical copies
of an album's tracks are combined automatically once the server has fingerprinted them,
which happens in the background after a scan, so they may show twice for a while after you
add a folder. Copies that differ (another format, or re-tagged files) stay separate; remove
one of the folders from **Settings → Music folders** if you don't want both. To tell copies
apart, right-click a track or album in the Player and choose **Info**: it shows where each
file lives.

### The first scan is slow

A first scan of a big library on a network drive takes time; later scans only look at what
changed. The Status tab shows progress. You can listen while it runs.

### No sound

- Player **Settings → Audio output**: pick the right device.
- Check the Mac's volume and the Player's volume.
- If **Bit-perfect output** is on and another app is using the same device, turn it off or
  quit the other app.

### Loud noise or static

**Turn the volume down first.** This happens when DSD (as DoP) or bit-perfect audio goes to
a device that doesn't support it. In the Player's Settings, set **DSD handling** back to
**Auto** and turn **Bit-perfect output** off. Please report which device you used.

### Album info lookup finds nothing

It's off until you turn it on (server **Settings → Album info**). If many albums show as
"not found", choose a less strict **Match strictness**, which looks them up again, or
click **Retry not found**. When there's no internet connection, the lookup pauses itself
and resumes later.

### The app looks stuck at startup

Each app shows a splash screen while it starts, for up to 20 seconds. If it never goes
away, quit the app (⌘Q) and open it again, and send the log if it happens again.

### Sending a log

Both apps keep a log of what happened, which is the most useful thing to send with a
report:

- Server: **Settings → Advanced → Reveal Logs in Finder** (`kahawai-server.log`)
- Player: **Settings → Logs → Reveal Logs in Finder** (`kahawai-player.log`)

Send the log along with what you did, what you expected, what happened, and the version
(in each app's **About**).

## FAQ

**Does Kahawai change my music files?**
No. It only reads them. What it learns (albums, genres, album info it looks up) goes in its
own database; your files, tags, covers and folders stay exactly as they are.

**Does my music or listening leave my home network?**
No. There are no accounts and no analytics. The one exception is optional: album info lookup
sends album and artist names (not your music) to MusicBrainz, and it's off unless you turn
it on.

**Can I listen away from home?**
Not in this version. The server has no login and no encryption, so it must stay on your
home network. Don't make it reachable from the internet.

**Can I run the Player on several Macs?**
Yes. Point each one at the same server address.

**Is there a Windows, iPhone or Android version?**
Not yet; this beta is for Macs.

**Why does macOS warn me when I first open the apps?**
The beta isn't signed and notarized with a paid Apple developer certificate yet. The steps
in [Open them the first time](#2-open-them-the-first-time) are needed once per app.

**Can I play my library in VLC?**
Yes, one playlist or album at a time, from any computer on your home network. See
[Playing your library in VLC](VLC.md).

**Can I bring my playlists?**
Yes: in the Player, **Playlists → Import M3U…** reads `.m3u` / `.m3u8` playlists whose files
are in your library.

**What's "bit-perfect" and should I turn it on?**
It sends the audio to your DAC exactly as it is in the file, with no volume control or sound
shaping. It's for dedicated DACs, especially MQA or DSD ones. If you listen through the Mac's
speakers or ordinary headphones, leave it off.

**Is Kahawai free?**
Yes. It's free software under the GNU Affero General Public License v3 or later. The source
code is at https://github.com/ksuayan/kahawai. See each app's **About** for the license,
the open-source notices and the disclaimers.

**How do I report a problem or an idea?**
Send it to the person who gave you the beta, with the log (see
[Sending a log](#sending-a-log)).
