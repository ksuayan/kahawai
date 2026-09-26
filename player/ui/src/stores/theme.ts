import { defineStore } from "pinia";
import { ref } from "vue";

export type Theme = "dark" | "light";
const KEY = "kahawai-player.theme";

function stored(): Theme | null {
  try {
    const v = localStorage.getItem(KEY);
    return v === "dark" || v === "light" ? v : null;
  } catch {
    return null;
  }
}

/** Dark-first: follow the OS light preference only until the user picks. */
function systemTheme(): Theme {
  try {
    return window.matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark";
  } catch {
    return "dark";
  }
}

/** Dark / light theme, applied as <html data-theme="…"> (see style.css). */
export const useThemeStore = defineStore("theme", () => {
  const theme = ref<Theme>(stored() ?? systemTheme());

  function apply(): void {
    document.documentElement.dataset.theme = theme.value;
  }

  function set(t: Theme): void {
    theme.value = t;
    apply();
    try {
      localStorage.setItem(KEY, t);
    } catch {
      /* storage unavailable: the choice lasts for this session */
    }
  }

  function toggle(): void {
    set(theme.value === "dark" ? "light" : "dark");
  }

  function init(): void {
    apply();
  }

  return { theme, set, toggle, init };
});
