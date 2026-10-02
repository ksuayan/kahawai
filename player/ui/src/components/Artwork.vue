<script setup lang="ts">
import { BookOpen, Music } from "lucide-vue-next";
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
    /** What stands in for a missing cover: a note for music, an open book for an audiobook. */
    placeholder?: "music" | "book";
  }>(),
  { size: 48, alt: "Artwork", radius: 6, fluid: false, placeholder: "music" },
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
    <div v-else class="flex size-full items-center justify-center text-faint" aria-hidden="true" :data-placeholder="placeholder">
      <component :is="placeholder === 'book' ? BookOpen : Music" :size="fluid ? 72 : size * 0.45" :stroke-width="1.5" />
    </div>
  </div>
</template>
