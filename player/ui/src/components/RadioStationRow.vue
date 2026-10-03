<script setup lang="ts">
import { Play, Radio } from "lucide-vue-next";
import { ref } from "vue";
import { joinParts } from "../lib/format";
import UiBadge from "../ui/UiBadge.vue";
import UiButton from "../ui/UiButton.vue";

/** One station: icon, name, where it is from, its codec and bitrate, and room for buttons; a row, or a tile in the grid. */
withDefaults(defineProps<{
  name: string;
  subtitle?: string;
  favicon?: string | null;
  codec?: string | null;
  bitrate?: number | null;
  playing?: boolean;
  disabledReason?: string | null;
  layout?: "list" | "grid";
}>(), { subtitle: undefined, favicon: null, codec: null, bitrate: null, playing: false, disabledReason: null, layout: "list" });
defineEmits<{ (e: "play"): void }>();

const iconFailed = ref(false);
</script>

<template>
  <li
    v-if="layout === 'grid'"
    class="flex flex-col gap-2 rounded-lg border border-line p-2 hover:bg-hover"
    :class="playing && 'border-accent bg-active'"
    data-testid="station-row"
    data-layout="grid"
  >
    <button
      type="button"
      class="flex aspect-square w-full items-center justify-center overflow-hidden rounded-md border-0 bg-raised p-0 text-dim disabled:opacity-45"
      :title="disabledReason ?? `Play ${name}`"
      :aria-label="`Play ${name}`"
      :disabled="!!disabledReason"
      data-testid="station-tile-play"
      @click="$emit('play')"
    >
      <img v-if="favicon && !iconFailed" :src="favicon" alt="" class="size-full object-contain p-3" loading="lazy" @error="iconFailed = true" />
      <Radio v-else class="size-10" />
    </button>
    <div class="min-w-0">
      <div class="truncate font-semibold" :class="playing && 'text-accent'" :title="name">{{ name }}</div>
      <div class="truncate text-xs text-dim" :title="subtitle">{{ subtitle }}</div>
    </div>
    <div class="flex min-h-7 flex-wrap items-center justify-between gap-1">
      <UiBadge v-if="codec || bitrate" class="max-[719px]:hidden" variant="default" data-testid="station-codec">
        {{ joinParts([codec, bitrate ? `${bitrate} kbps` : null]) }}
      </UiBadge>
      <span v-if="disabledReason" class="text-xs text-faint" data-testid="station-unplayable">{{ disabledReason }}</span>
      <span class="ml-auto flex items-center gap-0.5"><slot /></span>
    </div>
  </li>
  <li v-else class="flex items-center gap-3 rounded-md px-2 py-1.5 hover:bg-hover" :class="playing && 'bg-active'" data-testid="station-row">
    <div class="flex size-10 shrink-0 items-center justify-center overflow-hidden rounded-md bg-raised text-dim">
      <img v-if="favicon && !iconFailed" :src="favicon" alt="" class="size-10 object-contain" loading="lazy" @error="iconFailed = true" />
      <Radio v-else class="size-5" />
    </div>
    <div class="min-w-0 flex-1">
      <div class="truncate font-semibold" :class="playing && 'text-accent'">{{ name }}</div>
      <div class="truncate text-xs text-dim">{{ subtitle }}</div>
    </div>
    <UiBadge v-if="codec || bitrate" class="max-[719px]:hidden" variant="default" data-testid="station-codec">
      {{ joinParts([codec, bitrate ? `${bitrate} kbps` : null]) }}
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
