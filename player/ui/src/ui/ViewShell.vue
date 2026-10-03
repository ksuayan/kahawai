<script setup lang="ts">
import type { ViewName } from "../stores/nav";
import Breadcrumbs from "./Breadcrumbs.vue";

/** Page frame shared by every main view: padding, title, subtitle, actions.
 *  `full` is for card grids (Albums): with `auto-fill` columns, giving it
 *  the whole panel means more columns show up on a wide display instead of
 *  capping out at a fixed width regardless of window size. It also makes
 *  the shell a fixed-height flex column (header, then a `flex-1 min-h-0`
 *  body) instead of a naturally-growing block — a virtualized grid needs
 *  its own bounded, self-scrolling container rather than relying on some
 *  distant ancestor's scrollbar, the way every other (non-virtualized)
 *  view does today. It fills its parent with flex (`flex-1`), not `h-full`:
 *  percentage heights on flex children are resolved differently across
 *  engines (WebKit vs Chromium), and a wrong guess there is what makes the
 *  parent scroll too. So the parent must be a flex column (App.vue makes
 *  <main> one, and non-scrolling, while Albums is showing). */
withDefaults(
  defineProps<{
    title?: string;
    subtitle?: string;
    width?: "full" | "fluid" | "wide" | "medium" | "narrow";
    /** A page inside a section (an album, an artist…) shows the breadcrumb:
     *  this is its section, for when there is no trail to show. */
    section?: ViewName;
    /** The page's own title, the breadcrumb's last step. */
    crumb?: string;
  }>(),
  { width: "wide", section: undefined, crumb: undefined },
);
const widths = {
  full: "max-w-none",
  /** Uncapped width, but an ordinary scrolling page (for non-virtualized card grids). */
  fluid: "max-w-none",
  wide: "max-w-[1200px]",
  medium: "max-w-[900px]",
  narrow: "max-w-[640px]",
} as const;
</script>

<template>
  <div
    class="px-4 pb-10 pt-4 min-[720px]:px-6 min-[720px]:pt-5"
    :class="[widths[width], width === 'full' && 'flex min-h-0 flex-1 flex-col']"
  >
    <Breadcrumbs v-if="section" :section="section" :current="crumb" />
    <div
      v-if="title || $slots.actions"
      class="mb-4 flex flex-col gap-3 min-[720px]:flex-row min-[720px]:items-start min-[720px]:justify-between min-[720px]:gap-4"
      :class="width === 'full' && 'shrink-0'"
    >
      <div>
        <h2 v-if="title" class="heading-1 m-0 mb-1">{{ title }}</h2>
        <p v-if="subtitle" class="m-0 text-dim">{{ subtitle }}</p>
      </div>
      <div v-if="$slots.actions" class="flex flex-wrap items-center gap-2"><slot name="actions" /></div>
    </div>
    <slot />
  </div>
</template>
