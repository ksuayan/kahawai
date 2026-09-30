<script setup lang="ts">
import { Info } from "lucide-vue-next";
import { useSetupStore } from "../stores/setup";
import StatusTab from "./StatusTab.vue";
import SettingsTab from "./SettingsTab.vue";

const setup = useSetupStore();
const TABS = ["status", "settings"] as const;
</script>

<template>
  <div class="flex h-full flex-col">
    <div class="flex gap-1 border-b border-line px-8 pt-4">
      <button
        v-for="tab in TABS"
        :key="tab"
        type="button"
        class="-mb-px rounded-t-md border-b-2 px-3 py-2 text-[13px] capitalize transition-colors"
        :class="
          setup.activeTab === tab
            ? 'border-accent font-semibold text-fg'
            : 'border-transparent text-dim hover:text-fg'
        "
        @click="setup.activeTab = tab"
      >
        {{ tab }}
      </button>
      <button
        type="button"
        class="-mb-px ml-auto flex items-center gap-1.5 px-3 py-2 text-[13px] text-dim hover:text-fg"
        title="About Kahawai Server"
        data-testid="about-button"
        @click="setup.aboutOpen = true"
      >
        <Info class="size-4" /> About
      </button>
    </div>
    <div class="min-h-0 flex-1 overflow-y-auto">
      <StatusTab v-if="setup.activeTab === 'status'" />
      <SettingsTab v-else />
    </div>
  </div>
</template>
