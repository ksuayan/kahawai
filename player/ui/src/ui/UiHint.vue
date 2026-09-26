<script setup lang="ts">
/** Small explanatory paragraph under a settings heading. */
withDefaults(defineProps<{ tone?: "dim" | "warn" | "faint"; spaced?: boolean }>(), { tone: "dim", spaced: false });
</script>

<template>
  <p
    class="mb-2"
    :class="[
      spaced ? 'mt-2' : 'mt-1',
      { 'text-dim': tone === 'dim', 'text-[#ff9d97]': tone === 'warn', 'text-faint': tone === 'faint' },
    ]"
  >
    <slot />
  </p>
</template>
