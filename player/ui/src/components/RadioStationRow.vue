<script setup lang="ts">
import { Play, Radio } from "lucide-vue-next";
import { ref } from "vue";
import UiBadge from "../ui/UiBadge.vue";
import UiButton from "../ui/UiButton.vue";

/** One station: icon, name, where it is from, its codec and bitrate, and room for buttons. */
defineProps<{
  name: string;
  subtitle?: string;
  favicon?: string | null;
  codec?: string | null;
  bitrate?: number | null;
  playing?: boolean;
  disabledReason?: string | null;
}>();
defineEmits<{ (e: "play"): void }>();

const iconFailed = ref(false);
</script>

<template>
  <li class="flex items-center gap-3 rounded-md px-2 py-1.5 hover:bg-hover" :class="playing && 'bg-active'" data-testid="station-row">
    <div class="flex size-10 shrink-0 items-center justify-center overflow-hidden rounded-md bg-raised text-dim">
      <img v-if="favicon && !iconFailed" :src="favicon" alt="" class="size-10 object-contain" loading="lazy" @error="iconFailed = true" />
      <Radio v-else class="size-5" />
    </div>
    <div class="min-w-0 flex-1">
      <div class="truncate font-semibold" :class="playing && 'text-accent'">{{ name }}</div>
      <div class="truncate text-xs text-dim">{{ subtitle }}</div>
    </div>
    <UiBadge v-if="codec || bitrate" variant="default" data-testid="station-codec">
      {{ [codec, bitrate ? `${bitrate} kbps` : null].filter(Boolean).join(" · ") }}
    </UiBadge>
    <span v-if="disabledReason" class="text-xs text-faint" data-testid="station-unplayable">{{ disabledReason }}</span>
    <slot />
    <UiButton
      variant="icon"
      :title="disabledReason ?? `Play ${name}`"
      :aria-label="`Play ${name}`"
      :disabled="!!disabledReason"
      data-testid="station-play"
      @click="$emit('play')"
    >
      <Play />
    </UiButton>
  </li>
</template>
