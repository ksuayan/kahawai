<script setup lang="ts">
import { ChevronRight } from "lucide-vue-next";
import { computed, watch } from "vue";
import { crumbLabel, useNavStore, type Crumb, type ViewName } from "../stores/nav";

/**
 * Where this page is, and the way back: the trail of views that led here
 * ("Artists › Miles Davis › Kind of Blue"). Every step but the last goes
 * back to it. With no trail (the page was opened on its own), it shows the
 * page's section, then the page.
 */
const props = defineProps<{
  /** The section this page belongs to when there is no trail. */
  section: ViewName;
  /** This page's title, as the last step; it also names the step in the trail. */
  current?: string;
}>();
const nav = useNavStore();

const fromTrail = computed(() => nav.trail.length > 1);
const items = computed<{ label: string; crumb: Crumb }[]>(() => {
  const steps: Crumb[] = fromTrail.value ? nav.trail : [{ name: props.section }, { ...nav.view }];
  return steps.map((c, i) => ({
    crumb: c,
    label: i === steps.length - 1 && props.current ? props.current : crumbLabel(c),
  }));
});

watch(
  () => props.current,
  (title) => {
    if (title) nav.setLabel(nav.view, title);
  },
  { immediate: true },
);

function go(index: number): void {
  if (fromTrail.value) nav.goToCrumb(index);
  else nav.go(props.section);
}
</script>

<template>
  <nav aria-label="Breadcrumb" class="mb-3 shrink-0" data-testid="breadcrumbs">
    <ol class="m-0 flex list-none flex-wrap items-center gap-1 p-0 text-[13px]">
      <li v-for="(it, i) in items" :key="i" class="flex min-w-0 items-center gap-1">
        <ChevronRight v-if="i > 0" class="size-3.5 shrink-0 text-faint" aria-hidden="true" />
        <span
          v-if="i === items.length - 1"
          class="max-w-[320px] truncate font-semibold text-fg"
          aria-current="page"
          :title="it.label"
          data-testid="crumb-current"
        >{{ it.label }}</span>
        <button
          v-else
          type="button"
          class="max-w-[240px] truncate rounded-sm px-0.5 text-dim outline-none hover:text-fg hover:underline focus-visible:outline-2 focus-visible:outline-accent"
          :title="it.label"
          data-testid="crumb"
          @click="go(i)"
        >{{ it.label }}</button>
      </li>
    </ol>
  </nav>
</template>
