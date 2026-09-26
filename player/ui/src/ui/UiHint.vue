<script setup lang="ts">
/** Small explanatory paragraph under a settings heading. */
withDefaults(defineProps<{ tone?: "dim" | "warn" | "faint"; spaced?: boolean }>(), { tone: "dim", spaced: false });
</script>

<template>
  <p
    class="prose-text mb-3"
    :class="[
      spaced ? 'mt-2' : 'mt-1',
      { 'text-dim': tone === 'dim', 'text-danger-fg': tone === 'warn', 'text-faint': tone === 'faint' },
    ]"
  >
    <slot />
  </p>
</template>
