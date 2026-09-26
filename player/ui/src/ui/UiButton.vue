<script setup lang="ts">
import { computed } from "vue";
import { cn } from "./cn";

/**
 * The one button. `variant` picks the look; every other attribute
 * (disabled, title, @click, aria-*) falls through to the <button>.
 */
const props = withDefaults(
  defineProps<{
    variant?: "default" | "primary" | "danger" | "ghost" | "icon" | "nav";
    active?: boolean;
  }>(),
  { variant: "default", active: false },
);

const base =
  "inline-flex items-center justify-center gap-1.5 rounded-md text-[13px] transition-colors " +
  "focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-accent " +
  "disabled:opacity-45 disabled:cursor-default";

const variants: Record<string, string> = {
  default:
    "border border-line bg-raised text-fg px-3 py-[5px] enabled:hover:bg-hover",
  primary:
    "border border-accent bg-accent text-white px-3 py-[5px] enabled:hover:bg-accent-hover",
  danger: "border border-line bg-raised text-danger px-3 py-[5px] enabled:hover:bg-hover",
  ghost: "border border-transparent bg-transparent text-dim px-3 py-[5px] enabled:hover:bg-hover enabled:hover:text-fg",
  icon: "border-0 bg-transparent text-dim px-1.5 py-1 leading-none enabled:hover:bg-hover enabled:hover:text-fg",
  nav: "w-full justify-between border-0 bg-transparent px-3 py-[7px] text-left text-dim enabled:hover:bg-hover enabled:hover:text-fg",
};

const classes = computed(() =>
  cn(base, variants[props.variant], props.active && "!bg-active !text-fg font-semibold"),
);
</script>

<template>
  <button type="button" :class="classes" :data-variant="variant" :data-active="active || undefined">
    <slot />
  </button>
</template>
