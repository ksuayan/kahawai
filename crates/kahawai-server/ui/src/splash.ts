// Splash window: the app's name and version come from the shell (the
// query string), so they always match the running build.
import { invoke } from "@tauri-apps/api/core";
import "@fontsource/ibm-plex-serif/latin-400.css";
import "@fontsource/ibm-plex-serif/latin-ext-400.css";

const params = new URLSearchParams(window.location.search);
const name = params.get("name");
const version = params.get("version");
if (name) {
  document.getElementById("name")!.textContent = name;
  document.title = name;
}
if (version) document.getElementById("version")!.textContent = `Version ${version}`;

// Tell the shell once the artwork is decoded and painted: the splash's
// minimum display time counts from then, so it's seen even on a slow start.
const art = new Image();
art.src = "/splash.webp";
void art
  .decode()
  .catch(() => undefined)
  .then(() =>
    requestAnimationFrame(() =>
      requestAnimationFrame(() => {
        if ("__TAURI_INTERNALS__" in window) void invoke("splash_shown").catch(() => undefined);
      }),
    ),
  );
