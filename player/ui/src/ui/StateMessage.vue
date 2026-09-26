<script setup lang="ts">
import { computed } from "vue";
import { cn } from "./cn";

/** Loading / empty / error placeholder for a view's content area. */
const props = defineProps<{ kind: "loading" | "empty" | "error" }>();
const classes = computed(() =>
  cn(
    props.kind === "error"
      ? "mb-4 rounded-lg border border-danger/40 bg-danger/12 px-3.5 py-2.5 text-[#ff9d97]"
      : "py-10 text-center text-faint",
  ),
);
</script>

<template>
  <div :class="classes" :data-kind="kind" :role="kind === 'error' ? 'alert' : undefined">
    <slot />
  </div>
</template>
