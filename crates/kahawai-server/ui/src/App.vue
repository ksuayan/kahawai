<script setup lang="ts">
import { onMounted } from "vue";
import StatusView from "./components/StatusView.vue";
import WizardView from "./components/WizardView.vue";
import { useSetupStore } from "./stores/setup";

const setup = useSetupStore();

onMounted(async () => {
  await setup.init();
});
</script>

<template>
  <div class="flex h-full flex-col">
    <div v-if="setup.loading" class="flex h-full items-center justify-center text-faint">Starting…</div>
    <WizardView v-else-if="setup.view === 'wizard'" />
    <StatusView v-else />
  </div>
</template>
