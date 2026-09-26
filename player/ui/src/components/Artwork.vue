<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { artworkSrc } from "../api";

const props = withDefaults(
  defineProps<{
    hash?: string | null;
    size?: number;
    alt?: string;
    radius?: number;
    /** Fill the parent's width as a square instead of using `size`. */
    fluid?: boolean;
  }>(),
  { size: 48, alt: "Artwork", radius: 6, fluid: false },
);

const failed = ref(false);
const src = computed(() => artworkSrc(props.hash));
const showImg = computed(() => !!src.value && !failed.value);
// A different cover gets a fresh chance to load.
watch(src, () => (failed.value = false));

const boxStyle = computed(() => ({
  borderRadius: `${props.radius}px`,
  ...(props.fluid ? {} : { width: `${props.size}px`, height: `${props.size}px` }),
}));
</script>

<template>
  <div
    class="relative shrink-0 overflow-hidden bg-active"
    :class="fluid && 'aspect-square w-full'"
    :style="boxStyle"
  >
    <img
      v-if="showImg"
      :src="src"
      :alt="alt"
      draggable="false"
      class="block size-full object-cover"
      @error="failed = true"
    />
    <div v-else class="flex size-full items-center justify-center text-faint" aria-hidden="true">
      <svg viewBox="0 0 24 24" :width="fluid ? 72 : size * 0.45" :height="fluid ? 72 : size * 0.45">
        <path fill="currentColor" d="M12 3v10.55A4 4 0 1 0 14 17V7h4V3h-6z" />
      </svg>
    </div>
  </div>
</template>
