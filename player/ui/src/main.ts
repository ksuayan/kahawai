import { createApp } from "vue";
import { createPinia } from "pinia";
import App from "./App.vue";
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
import { useThemeStore } from "./stores/theme";

const app = createApp(App);
app.use(createPinia());
useThemeStore().init(); // before mount: no flash of the wrong theme
app.mount("#app");
