#!/usr/bin/env python3
"""Merge the Kahawai iOS background-audio entries into the generated Info.plist.

iOS suspends an app's audio when it backgrounds unless the app declares the
`audio` background mode. Tauri's generated Xcode project does not expose this,
so `scripts/build-ios.sh` / `scripts/start-dev-ios.sh` run this after
`cargo tauri ios init` and before the Xcode build:

    python3 scripts/ios-plist-overlay.py [path/to/Info.plist]

Default path: player/src-tauri/gen/ios/<app>/Info.plist (the generated,
gitignored project). Idempotent: entries already present are left alone, so
running it twice is a no-op. Exits non-zero if the plist is missing (run
`cargo tauri ios init` first).
"""
import plistlib
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# key -> value to ensure present. Values merge by union for lists.
OVERLAY = {
    # Background audio: keeps the Rust audio thread audible when the app
    # backgrounds or the screen locks. Paired with the AVAudioSession in
    # player/src-tauri/src/ios_audio.rs (Playback category, active while
    # playing).
    "UIBackgroundModes": ["audio"],
}


def find_plist(explicit: str | None) -> Path:
    if explicit:
        return Path(explicit)
    gen = ROOT / "player" / "src-tauri" / "gen" / "ios"
    found = sorted(gen.rglob("Info.plist")) if gen.is_dir() else []
    if not found:
        print(
            "error: no Info.plist under player/src-tauri/gen/ios; "
            "run `cargo tauri ios init` first",
            file=sys.stderr,
        )
        sys.exit(1)
    if len(found) > 1:
        print(
            f"warning: multiple Info.plist files; using {found[0]}",
            file=sys.stderr,
        )
    return found[0]


def main() -> None:
    plist_path = find_plist(sys.argv[1] if len(sys.argv) > 1 else None)
    with plist_path.open("rb") as f:
        plist = plistlib.load(f)

    changed = False
    for key, value in OVERLAY.items():
        if isinstance(value, list):
            current = plist.get(key)
            if not isinstance(current, list):
                plist[key] = list(value)
                changed = True
            else:
                for item in value:
                    if item not in current:
                        current.append(item)
                        changed = True
        elif plist.get(key) != value:
            plist[key] = value
            changed = True

    if changed:
        with plist_path.open("wb") as f:
            plistlib.dump(plist, f)
        print(f"updated {plist_path}")
    else:
        print(f"no changes needed in {plist_path}")


if __name__ == "__main__":
    main()
