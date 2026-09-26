<script setup lang="ts">
import { computed } from "vue";
import { useNavStore, type NavState } from "../stores/nav";
import { useQueueStore } from "../stores/queue";
import UiButton from "../ui/UiButton.vue";

const nav = useNavStore();
const queue = useQueueStore();

const items: { name: NavState["name"]; label: string; key: string }[] = [
  { name: "albums", label: "Albums", key: "1" },
  { name: "artists", label: "Artists", key: "2" },
  { name: "playlists", label: "Playlists", key: "3" },
  { name: "search", label: "Search", key: "4" },
  { name: "queue", label: "Queue", key: "5" },
];

const active = computed(() => {
  const v = nav.view.name;
  if (v === "album") return "albums";
  if (v === "artist") return "artists";
  if (v === "playlist") return "playlists";
  return v;
});
</script>

<template>
  <aside class="flex w-52 shrink-0 flex-col border-r border-line bg-raised px-2 py-3">
    <div class="px-3 pb-3 pt-1 text-[15px] font-bold tracking-[0.2px]">Music</div>
    <nav class="flex flex-col gap-0.5" aria-label="Library">
      <UiButton
        v-for="item in items"
        :key="item.name"
        variant="nav"
        :active="active === item.name"
        :aria-current="active === item.name ? 'page' : undefined"
        @click="nav.go(item.name)"
      >
        <span>{{ item.label }}</span>
        <span
          v-if="item.name === 'queue' && queue.tracks.length > 0"
          class="rounded-full bg-active px-2 py-px text-[11px] font-normal text-dim"
          data-testid="queue-count"
        >
          {{ queue.tracks.length }}
        </span>
      </UiButton>
    </nav>
    <div class="flex-1" />
    <UiButton
      variant="nav"
      :active="active === 'settings'"
      :aria-current="active === 'settings' ? 'page' : undefined"
      @click="nav.go('settings')"
    >
      <span>Settings</span>
    </UiButton>
  </aside>
</template>
