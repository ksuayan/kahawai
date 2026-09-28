<script setup lang="ts">
/** Page frame shared by every main view: padding, title, subtitle, actions.
 *  `full` is for card grids (Albums): with `auto-fill` columns, giving it
 *  the whole panel means more columns show up on a wide display instead of
 *  capping out at a fixed width regardless of window size. */
withDefaults(
  defineProps<{ title?: string; subtitle?: string; width?: "full" | "wide" | "medium" | "narrow" }>(),
  { width: "wide" },
);
const widths = {
  full: "max-w-none",
  wide: "max-w-[1200px]",
  medium: "max-w-[900px]",
  narrow: "max-w-[640px]",
} as const;
</script>

<template>
  <div class="px-6 pb-10 pt-5" :class="widths[width]">
    <div v-if="title || $slots.actions" class="mb-4 flex items-start justify-between gap-4">
      <div>
        <h2 v-if="title" class="heading-1 m-0 mb-1">{{ title }}</h2>
        <p v-if="subtitle" class="m-0 text-dim">{{ subtitle }}</p>
      </div>
      <div v-if="$slots.actions" class="flex items-center gap-2"><slot name="actions" /></div>
    </div>
    <slot />
  </div>
</template>
