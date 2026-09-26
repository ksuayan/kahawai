<script setup lang="ts">
import { SlidersVertical } from "lucide-vue-next";
import { computed, ref } from "vue";
import { useDspStore } from "../stores/dsp";
import { usePlayerStore } from "../stores/player";
import UiButton from "../ui/UiButton.vue";
import EqDialog from "./EqDialog.vue";

/** The EQ button next to shuffle/repeat: opens the interactive EQ editor. */
const props = withDefaults(defineProps<{ size?: "md" | "lg"; disabled?: boolean }>(), { size: "md", disabled: false });
const dsp = useDspStore();
const player = usePlayerStore();
const open = ref(false);
const unsupported = computed(() => player.isExclusive);

const title = computed(() =>
  unsupported.value
    ? "EQ is not supported for this stream type."
    : dsp.eqEnabled
      ? `EQ: ${dsp.activePreset?.name ?? "Custom"}`
      : "EQ off",
);
</script>

<template>
  <UiButton
    variant="icon"
    :size="props.size"
    :pressed="dsp.eqEnabled && !unsupported"
    :class="unsupported ? 'opacity-45' : ''"
    :title="title"
    aria-label="Equalizer"
    :disabled="disabled"
    data-testid="eq-button"
    @click="open = true"
  >
    <SlidersVertical />
  </UiButton>
  <EqDialog v-model:open="open" />
</template>
