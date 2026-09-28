<script setup lang="ts">
import UiHint from "../ui/UiHint.vue";
import { useSetupStore } from "../stores/setup";

const setup = useSetupStore();
</script>

<template>
  <div>
    <h2 class="heading-1 mb-2">Welcome to Kahawai Server</h2>
    <p class="prose-text mb-4">
      This sets up where your music lives and where the catalog database is kept. It takes about a
      minute — you can change any of this later from the status view.
    </p>
    <UiHint tone="faint">Configuration will be saved to {{ setup.configPath }}</UiHint>
  </div>
</template>
