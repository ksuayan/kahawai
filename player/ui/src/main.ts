import { createApp } from "vue";
import { createPinia } from "pinia";
import App from "./App.vue";
// IBM Plex Sans, bundled (OFL) so the UI looks the same on every machine and
// works offline. Latin + Latin Extended; other scripts use the system fallback.
import "@fontsource/ibm-plex-sans/latin-400.css";
import "@fontsource/ibm-plex-sans/latin-500.css";
import "@fontsource/ibm-plex-sans/latin-600.css";
import "@fontsource/ibm-plex-sans/latin-700.css";
import "@fontsource/ibm-plex-sans/latin-ext-400.css";
import "@fontsource/ibm-plex-sans/latin-ext-500.css";
import "@fontsource/ibm-plex-sans/latin-ext-600.css";
import "@fontsource/ibm-plex-sans/latin-ext-700.css";
import "./style.css";

const app = createApp(App);
app.use(createPinia());
app.mount("#app");
