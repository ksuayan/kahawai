<script setup lang="ts">
import { computed, ref } from "vue";
import { artworkSrc } from "../api";

const props = withDefaults(
  defineProps<{
    hash?: string | null;
    size?: number;
    alt?: string;
    radius?: number;
  }>(),
  { size: 48, alt: "Artwork", radius: 6 },
);

const failed = ref(false);
const src = computed(() => artworkSrc(props.hash));
const showImg = computed(() => !!src.value && !failed.value);
</script>

<template>
  <div
    class="artwork"
    :style="{ width: size + 'px', height: size + 'px', borderRadius: radius + 'px' }"
  >
    <img
      v-if="showImg"
      :src="src"
      :alt="alt"
      draggable="false"
      @error="failed = true"
    />
    <div v-else class="fallback" aria-hidden="true">
      <svg viewBox="0 0 24 24" :width="size * 0.45" :height="size * 0.45">
        <path
          fill="currentColor"
          d="M12 3v10.55A4 4 0 1 0 14 17V7h4V3h-6z"
        />
      </svg>
    </div>
  </div>
</template>

<style scoped>
.artwork {
  flex-shrink: 0;
  overflow: hidden;
  background: var(--bg-active);
  position: relative;
}

.artwork img {
  width: 100%;
  height: 100%;
  object-fit: cover;
  display: block;
}

.fallback {
  width: 100%;
  height: 100%;
  display: flex;
  align-items: center;
  justify-content: center;
  color: var(--text-faint);
}
</style>
