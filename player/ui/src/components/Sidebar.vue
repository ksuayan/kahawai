<script setup lang="ts">
import { computed } from "vue";
import { useNavStore, type NavState } from "../stores/nav";
import { useQueueStore } from "../stores/queue";

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
  <aside class="sidebar">
    <div class="brand">Music</div>
    <nav>
      <button
        v-for="item in items"
        :key="item.name"
        class="nav-item"
        :class="{ active: active === item.name }"
        @click="nav.go(item.name)"
      >
        <span class="label">{{ item.label }}</span>
        <span v-if="item.name === 'queue' && queue.tracks.length > 0" class="count">
          {{ queue.tracks.length }}
        </span>
      </button>
    </nav>
    <div class="spacer" />
    <button class="nav-item settings" :class="{ active: active === 'settings' }" @click="nav.go('settings')">
      <span class="label">Settings</span>
    </button>
  </aside>
</template>

<style scoped>
.sidebar {
  width: 208px;
  flex-shrink: 0;
  background: var(--bg-raised);
  border-right: 1px solid var(--border);
  display: flex;
  flex-direction: column;
  padding: 12px 8px;
}

.brand {
  font-size: 15px;
  font-weight: 700;
  padding: 4px 12px 12px;
  letter-spacing: 0.2px;
}

nav {
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.spacer {
  flex: 1;
}

.nav-item {
  display: flex;
  align-items: center;
  justify-content: space-between;
  background: transparent;
  border: none;
  border-radius: 6px;
  padding: 7px 12px;
  text-align: left;
  color: var(--text-dim);
  width: 100%;
}

.nav-item:hover {
  background: var(--bg-hover);
  color: var(--text);
}

.nav-item.active {
  background: var(--bg-active);
  color: var(--text);
  font-weight: 600;
}

.count {
  font-size: 11px;
  background: var(--bg-active);
  border-radius: 10px;
  padding: 1px 8px;
  color: var(--text-dim);
}
</style>
