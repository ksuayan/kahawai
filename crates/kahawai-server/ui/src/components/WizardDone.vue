<script setup lang="ts">
import UiButton from "../ui/UiButton.vue";
import UiHint from "../ui/UiHint.vue";
import { useSetupStore } from "../stores/setup";

const setup = useSetupStore();
</script>

<template>
  <div>
    <h2 class="heading-1 mb-2">Configuration saved</h2>
    <p class="prose-text mb-4">Configuration saved to {{ setup.configPath }}</p>

    <UiHint v-if="setup.startError" tone="warn">{{ setup.startError }}</UiHint>

    <div class="flex flex-wrap gap-2">
      <UiButton variant="primary" @click="setup.startServerAndContinue()">Start Server & Continue</UiButton>
      <UiButton @click="setup.revealConfig()">Reveal in Finder</UiButton>
      <UiButton variant="danger" @click="setup.quit()">Quit</UiButton>
    </div>
  </div>
</template>
