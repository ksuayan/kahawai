# Remote access over Tailscale

Kahawai Server is LAN-first by design: it binds `127.0.0.1` by default and
treats any wider listen scope as a deliberate choice. To reach your library
away from home — from a phone on a hotspot, a laptop at a café — put the
server and your players on the same [Tailscale](https://tailscale.com)
tailnet instead of exposing the server to the internet.

## What you need

- The stock Tailscale client on each device (macOS, iOS, Android, …).
- Every device signed in to the **same tailnet**.
- Kahawai Server's listen scope set to **Tailscale only** (server app →
  Review → advanced, or the Settings tab).

## How it works

With "Tailscale only", the server binds the machine's tailnet IPv4
(`100.x.y.z`) at startup and refuses to start if no tailnet address is
found — it never silently falls back to `0.0.0.0`. The server's home page
and the desktop app's Status tab show the detected tailnet IP and the
MagicDNS name when one is known.

In the player's Server settings, enter the MagicDNS name, e.g.

```
http://kahawai-mac.tail12345.ts.net:8080
```

The player accepts MagicDNS names anywhere a server URL goes. If the name
doesn't resolve, Settings says so explicitly ("Tailscale doesn't seem to
be running on this device") instead of a bare "unreachable".

## What this does and doesn't do

- Traffic between your devices is encrypted by Tailscale (WireGuard).
  Kahawai itself still speaks plain HTTP on the tailnet — the tailnet is
  the trusted network here.
- Bearer-token auth still applies: pair each player with the server's
  pairing token (or sign in, once user accounts exist).
- Tailscale must be **running** on every device, every time. If Tailscale
  is off, the server won't start (in "Tailscale only" mode) and players
  can't resolve the MagicDNS name. This is deliberate: fail closed, not
  open.

## Example tailnet ACL

Tailscale's default ACL allows all devices on the tailnet to reach each
other, which is all Kahawai needs. If you lock your tailnet down with a
custom ACL, allow TCP 8080 (or your configured port) to the server.
Example `acls` fragment for the
[Tailscale ACL syntax](https://tailscale.com/kb/1337/acl-syntax):

```json
{
  "tagOwners": {
    "tag:kahawai-server": ["autogroup:member"]
  },
  "acls": [
    {
      "action": "accept",
      "src": ["autogroup:member"],
      "dst": ["tag:kahawai-server:8080"]
    }
  ]
}
```

Tag the Mac running Kahawai Server with `tag:kahawai-server`; players on
the tailnet (any `autogroup:member` device) can then reach port 8080 on it
and nothing else needs opening.
