<script setup lang="ts">
import { computed, type Component } from "vue";
import { Disc3, ListMusic, ListOrdered, MicVocal, Search, Settings } from "lucide-vue-next";
import { useNavStore, type NavState } from "../stores/nav";
import { useQueueStore } from "../stores/queue";
import UiButton from "../ui/UiButton.vue";

const nav = useNavStore();
const queue = useQueueStore();

const items: { name: NavState["name"]; label: string; key: string; icon: Component }[] = [
  { name: "albums", label: "Albums", key: "1", icon: Disc3 },
  { name: "artists", label: "Artists", key: "2", icon: MicVocal },
  { name: "playlists", label: "Playlists", key: "3", icon: ListMusic },
  { name: "search", label: "Search", key: "4", icon: Search },
  { name: "queue", label: "Queue", key: "5", icon: ListOrdered },
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
        <span class="flex items-center gap-2.5">
          <component :is="item.icon" class="size-4" />{{ item.label }}
        </span>
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
      <span class="flex items-center gap-2.5"><Settings class="size-4" />Settings</span>
    </UiButton>
  </aside>
</template>
