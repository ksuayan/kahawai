// First: built-ins older WebViews lack (lib/polyfills), before anything uses them.
import "./lib/polyfills";
import { createApp } from "vue";
import { createPinia } from "pinia";
import App from "./App.vue";
import { watchNativeInsets } from "./lib/insets";
// IBM Plex Sans, bundled (OFL) so the UI looks the same on every machine and
// works offline. Latin + Latin Extended; other scripts use the system fallback.
import "@fontsource/ibm-plex-sans/latin-400.css";
import "@fontsource/ibm-plex-sans/latin-600.css";
import "@fontsource/ibm-plex-sans/latin-ext-400.css";
import "@fontsource/ibm-plex-sans/latin-ext-600.css";
// IBM Plex Serif (prose only: descriptions, empty states), 400/600 like Sans.
import "@fontsource/ibm-plex-serif/latin-400.css";
import "@fontsource/ibm-plex-serif/latin-600.css";
import "@fontsource/ibm-plex-serif/latin-ext-400.css";
import "@fontsource/ibm-plex-serif/latin-ext-600.css";
import "./style.css";

// Android: the system bars' real size, where the WebView reports none (lib/insets).
watchNativeInsets();
import { installNativeMenuGuard } from "./lib/nativeMenu";
import { loadUiState } from "./lib/uiState";
import { useDeveloperStore } from "./stores/developer";
import { useThemeStore } from "./stores/theme";

async function start(): Promise<void> {
  // Saved preferences first: the stores read them as they're created.
  await loadUiState();
  const app = createApp(App);
  app.use(createPinia());
  useThemeStore().init(); // before mount: no flash of the wrong theme
  // No WebKit menu (Reload, Inspect) in a release build unless the user asked.
  const developer = useDeveloperStore();
  installNativeMenuGuard(developer.nativeMenuAllowed);
  void developer.load();
  app.mount("#app");
}

void start();
