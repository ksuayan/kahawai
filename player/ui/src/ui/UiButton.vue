<script setup lang="ts">
import { computed } from "vue";
import { cn } from "./cn";

/**
 * The one button. `variant` picks the look; every other attribute
 * (disabled, title, @click, aria-*) falls through to the <button>.
 *
 * - `pressed`: toggle state (shuffle, repeat…) — accent tint + aria-pressed.
 * - `active`: the current item in a list of buttons (sidebar nav).
 * - `size`: icon buttons only (transport controls).
 *
 * Colour utilities are chosen per state rather than layered, so no two
 * classes ever compete for the same CSS property.
 */
const props = withDefaults(
  defineProps<{
    variant?: "default" | "primary" | "danger" | "ghost" | "icon" | "icon-danger" | "icon-strong" | "nav";
    size?: "default" | "md" | "lg" | "xl";
    pressed?: boolean;
    active?: boolean;
  }>(),
  { variant: "default", size: "default", pressed: false, active: false },
);

const base =
  "inline-flex items-center gap-1.5 rounded-md text-[13px] transition-colors " +
  "focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-accent " +
  "disabled:opacity-45 disabled:cursor-default [&_svg]:shrink-0";

const box = "justify-center border px-3 py-[5px] [&_svg]:size-4";

const iconSizes: Record<string, string> = {
  default: "px-1.5 py-1 [&_svg]:size-4",
  md: "px-2.5 py-1 text-[15px] [&_svg]:size-[18px]",
  lg: "px-3.5 py-2 text-lg [&_svg]:size-5",
  xl: "px-3.5 py-2 text-[22px] [&_svg]:size-6",
};

const idleGhost = "bg-transparent text-dim enabled:hover:bg-hover enabled:hover:text-fg";

const classes = computed(() => {
  const { variant, size, pressed, active } = props;
  switch (variant) {
    case "primary":
      return cn(base, box, "border-accent bg-accent text-white enabled:hover:bg-accent-hover");
    case "danger":
      return cn(base, box, "border-line bg-raised text-danger enabled:hover:bg-hover");
    case "ghost":
      return cn(base, box, "border-transparent", idleGhost);
    case "icon":
    case "icon-danger":
    case "icon-strong":
      return cn(
        base,
        "justify-center border-0 leading-none",
        iconSizes[size],
        pressed
          ? "bg-accent/15 text-accent"
          : variant === "icon-danger"
            ? "bg-transparent text-dim enabled:hover:bg-hover enabled:hover:text-danger"
            : variant === "icon-strong"
              ? "bg-transparent text-fg enabled:hover:bg-hover"
              : idleGhost,
      );
    case "nav":
      return cn(
        base,
        "w-full justify-between border-0 px-3 py-[7px] text-left",
        active ? "bg-active font-semibold text-fg" : cn(idleGhost),
      );
    default:
      return cn(base, box, "border-line bg-raised text-fg enabled:hover:bg-hover");
  }
});
</script>

<template>
  <button
    type="button"
    :class="classes"
    :data-variant="variant"
    :data-active="active || undefined"
    :aria-pressed="variant.startsWith('icon') ? (pressed ? 'true' : undefined) : undefined"
  >
    <slot />
  </button>
</template>
