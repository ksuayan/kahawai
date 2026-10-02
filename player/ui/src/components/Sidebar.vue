<script setup lang="ts">
import { computed, type Component } from "vue";
import { BookOpen, Disc3, Info, Tags, ListMusic, ListOrdered, MicVocal, Moon, Radio, Search, Settings, Sun } from "lucide-vue-next";
import { useNavStore, type NavState } from "../stores/nav";
import { useOverlaysStore } from "../stores/overlays";
import { useQueueStore } from "../stores/queue";
import { useThemeStore } from "../stores/theme";
import UiButton from "../ui/UiButton.vue";

const nav = useNavStore();
const queue = useQueueStore();
const theme = useThemeStore();
const overlays = useOverlaysStore();

const items: { name: NavState["name"]; label: string; key: string; icon: Component }[] = [
  { name: "albums", label: "Albums", key: "1", icon: Disc3 },
  { name: "artists", label: "Artists", key: "2", icon: MicVocal },
  { name: "genres", label: "Genres", key: "g", icon: Tags },
  { name: "playlists", label: "Playlists", key: "3", icon: ListMusic },
  { name: "audiobooks", label: "Audiobooks", key: "7", icon: BookOpen },
  { name: "radio", label: "Radio", key: "8", icon: Radio },
  { name: "search", label: "Search", key: "4", icon: Search },
  { name: "queue", label: "Queue", key: "5", icon: ListOrdered },
];

const active = computed(() => {
  const v = nav.view.name;
  if (v === "album") return "albums";
  if (v === "artist") return "artists";
  if (v === "genre") return "genres";
  if (v === "playlist") return "playlists";
  if (v === "audiobook") return "audiobooks";
  return v;
});
</script>

<template>
  <aside class="flex w-52 shrink-0 flex-col border-r border-line bg-raised px-2 py-3">
    <div class="px-3 pb-3 pt-1 heading-3">Music</div>
    <nav class="flex flex-col gap-0.5" aria-label="Library">
      <UiButton
        v-for="item in items"
        :key="item.name"
        variant="nav"
        :active="active === item.name"
        :aria-current="active === item.name ? 'page' : undefined"
        @click="nav.go(item.name)"
      >
        <span class="flex items-center gap-2.5">
          <component :is="item.icon" class="size-4" />{{ item.label }}
        </span>
        <span
          v-if="item.name === 'queue' && queue.tracks.length > 0"
          class="rounded-full bg-active px-2 py-px text-[11px] text-dim"
          data-testid="queue-count"
        >
          {{ queue.tracks.length }}
        </span>
      </UiButton>
    </nav>
    <div class="flex-1" />
    <div class="flex items-center gap-1">
      <UiButton
        variant="nav"
        class="min-w-0 flex-1"
        :active="active === 'settings'"
        :aria-current="active === 'settings' ? 'page' : undefined"
        @click="nav.go('settings')"
      >
        <span class="flex items-center gap-2.5"><Settings class="size-4" />Settings</span>
      </UiButton>
      <UiButton
        variant="icon"
        size="md"
        :title="theme.theme === 'dark' ? 'Switch to light theme' : 'Switch to dark theme'"
        :aria-label="theme.theme === 'dark' ? 'Switch to light theme' : 'Switch to dark theme'"
        data-testid="theme-toggle"
        @click="theme.toggle()"
      >
        <Sun v-if="theme.theme === 'dark'" />
        <Moon v-else />
      </UiButton>
      <UiButton
        variant="icon"
        size="md"
        title="About Kahawai Player"
        aria-label="About Kahawai Player"
        data-testid="about-button"
        @click="overlays.openAbout()"
      >
        <Info />
      </UiButton>
    </div>
  </aside>
</template>
