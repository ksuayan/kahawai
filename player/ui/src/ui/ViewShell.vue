<script setup lang="ts">
/** Page frame shared by every main view: padding, title, subtitle, actions. */
withDefaults(defineProps<{ title?: string; subtitle?: string; width?: "wide" | "medium" | "narrow" }>(), {
  width: "wide",
});
const widths = { wide: "max-w-[1200px]", medium: "max-w-[900px]", narrow: "max-w-[640px]" } as const;
</script>

<template>
  <div class="px-6 pb-10 pt-5" :class="widths[width]">
    <div v-if="title || $slots.actions" class="mb-4 flex items-start justify-between gap-4">
      <div>
        <h2 v-if="title" class="m-0 mb-1 text-xl font-semibold">{{ title }}</h2>
        <p v-if="subtitle" class="m-0 text-dim">{{ subtitle }}</p>
      </div>
      <div v-if="$slots.actions" class="flex items-center gap-2"><slot name="actions" /></div>
    </div>
    <slot />
  </div>
</template>
