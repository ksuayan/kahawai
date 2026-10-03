<script setup lang="ts">
import { computed, type Component } from "vue";
import { BookOpen, Disc3, ListOrdered, Podcast, Radio, Search, Settings } from "lucide-vue-next";
import { SECTION_LABELS, useNavStore, type ViewName } from "../stores/nav";
import { useQueueStore } from "../stores/queue";

const nav = useNavStore();
const queue = useQueueStore();

interface Tab {
  id: string;
  label: string;
  icon: Component;
  sections: ViewName[];
  go: () => void;
}

const tabs: Tab[] = [
  {
    id: "library",
    label: "Library",
    icon: Disc3,
    sections: ["albums", "artists", "genres", "playlists"],
    go: () => nav.go("albums"),
  },
  {
    id: "audiobooks",
    label: SECTION_LABELS.audiobooks,
    icon: BookOpen,
    sections: ["audiobooks"],
    go: () => nav.go("audiobooks"),
  },
  {
    id: "podcasts",
    label: SECTION_LABELS.podcasts,
    icon: Podcast,
    sections: ["podcasts"],
    go: () => nav.go("podcasts"),
  },
  {
    id: "radio",
    label: SECTION_LABELS.radio,
    icon: Radio,
    sections: ["radio"],
    go: () => nav.go("radio"),
  },
  {
    id: "search",
    label: SECTION_LABELS.search,
    icon: Search,
    sections: ["search"],
    go: () => nav.go("search"),
  },
  {
    id: "queue",
    label: SECTION_LABELS.queue,
    icon: ListOrdered,
    sections: ["queue"],
    go: () => nav.go("queue"),
  },
  {
    id: "settings",
    label: SECTION_LABELS.settings,
    icon: Settings,
    sections: ["settings"],
    go: () => nav.go("settings"),
  },
];

const activeTab = computed(() => {
  const s = nav.section;
  return tabs.find((t) => t.sections.includes(s))?.id ?? "library";
});
</script>

<template>
  <nav
    class="pb-safe relative z-20 border-t border-line bg-raised"
    aria-label="Primary"
    data-testid="phone-tab-bar"
  >
    <!-- Icon-only: seven tabs fit a 360 px phone (~51 px each); labels live in aria-label. -->
    <div class="grid grid-cols-7">
      <button
        v-for="tab in tabs"
        :key="tab.id"
        type="button"
        class="phone-tab-btn relative flex min-h-[56px] items-center justify-center border-0 bg-transparent"
        :class="activeTab === tab.id ? 'text-accent' : 'text-dim'"
        :aria-label="tab.label"
        :aria-current="activeTab === tab.id ? 'page' : undefined"
        :data-testid="`tab-${tab.id}`"
        @click="tab.go()"
      >
        <span
          v-if="activeTab === tab.id"
          class="absolute inset-x-6 top-0 h-0.5 rounded-full bg-accent"
          aria-hidden="true"
        />
        <component :is="tab.icon" class="size-6" />
        <span
          v-if="tab.id === 'queue' && queue.tracks.length > 0"
          class="absolute right-1 top-1 rounded-full bg-active px-1.5 py-px text-[10px] text-dim"
          data-testid="queue-count"
        >
          {{ queue.tracks.length }}
        </span>
      </button>
    </div>
  </nav>
</template>
