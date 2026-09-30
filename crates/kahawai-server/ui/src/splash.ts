// Splash window: the app's name and version come from the shell (the
// query string), so they always match the running build.
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
