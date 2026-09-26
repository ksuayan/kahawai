<script setup lang="ts">
import { computed } from "vue";
import { cn } from "./cn";

const props = withDefaults(
  defineProps<{ variant?: "default" | "accent" | "ok" | "bad" | "danger" }>(),
  { variant: "default" },
);

const variants: Record<string, string> = {
  default: "text-dim border-line",
  accent: "text-accent border-accent/50",
  ok: "text-ok bg-ok/15 border-transparent",
  bad: "text-[#ff9d97] bg-danger/15 border-transparent",
  danger: "text-danger border-danger/50",
};
const classes = computed(() =>
  cn(
    "inline-block shrink-0 whitespace-nowrap rounded border px-1.5 py-0.5 text-[10px] font-semibold tracking-[0.4px]",
    variants[props.variant],
  ),
);
</script>

<template>
  <span :class="classes" :data-variant="variant"><slot /></span>
</template>
