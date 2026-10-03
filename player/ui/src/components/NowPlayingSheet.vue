<script setup lang="ts">
import { DialogContent, DialogOverlay, DialogPortal, DialogRoot, DialogTitle } from "reka-ui";
import { computed, ref } from "vue";
import { useAudiobooksStore } from "../stores/audiobooks";
import { usePlayerStore } from "../stores/player";
import { trackTitle } from "../types";
import AudiobookControls from "./AudiobookControls.vue";
import NowPlayingContent from "./NowPlayingContent.vue";
import SeekBar from "./SeekBar.vue";
import TransportControls from "./TransportControls.vue";

const props = defineProps<{ open: boolean }>();
const emit = defineEmits<{ (e: "update:open", v: boolean): void }>();

const player = usePlayerStore();
const books = useAudiobooksStore();

const track = computed(() => player.currentTrack);
const sheetTitle = computed(() =>
  books.isActive && books.active ? books.active.title : track.value ? trackTitle(track.value) : "Now playing",
);

/** Drag-to-dismiss: a downward drag on the handle translates the sheet; past a third of the viewport it closes. */
const dragY = ref(0);
const dragging = ref(false);
let startY = 0;

function onPointerDown(e: PointerEvent): void {
  dragging.value = true;
  startY = e.clientY;
  (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
}

function onPointerMove(e: PointerEvent): void {
  if (!dragging.value) return;
  dragY.value = Math.max(0, e.clientY - startY);
}

function onPointerUp(e: PointerEvent): void {
  if (!dragging.value) return;
  dragging.value = false;
  // Dragged past a third of the viewport: dismiss.
  const shouldClose = dragY.value > window.innerHeight / 3;
  dragY.value = 0;
  if (shouldClose) emit("update:open", false);
}

function close(): void {
  emit("update:open", false);
}
</script>

<template>
  <DialogRoot :open="props.open" @update:open="(v) => emit('update:open', v)">
    <DialogPortal>
      <DialogOverlay class="fixed inset-0 z-50 bg-black/55" data-testid="sheet-scrim" />
      <DialogContent
        class="pt-safe fixed inset-x-0 bottom-0 z-50 max-h-[92dvh] overflow-y-auto rounded-t-xl border-t border-line bg-raised shadow-float outline-none"
        :style="dragging ? { transform: `translateY(${dragY}px)` } : undefined"
        :class="{ 'transition-transform duration-200': !dragging }"
        aria-describedby="sheet-desc"
        data-testid="now-playing-sheet"
      >
        <DialogTitle class="sr-only">{{ sheetTitle }}</DialogTitle>
        <p id="sheet-desc" class="sr-only">Now playing. Swipe down or press Escape to close.</p>
        <!-- drag handle -->
        <div
          class="sticky top-0 flex justify-center bg-raised pb-1 pt-2"
          data-testid="sheet-handle"
          role="button"
          tabindex="0"
          aria-label="Drag down to close"
          @pointerdown="onPointerDown"
          @pointermove="onPointerMove"
          @pointerup="onPointerUp"
          @keydown.escape="close()"
        >
          <span class="h-1 w-10 rounded-full bg-active" aria-hidden="true" />
        </div>
        <div class="px-4 pb-6">
          <!-- transport first: the reason the sheet was opened -->
          <div class="mb-4 flex flex-col gap-1">
            <AudiobookControls v-if="books.isActive" />
            <TransportControls :disabled="!track" />
            <SeekBar :disabled="!track" />
          </div>
          <NowPlayingContent compact />
        </div>
      </DialogContent>
    </DialogPortal>
  </DialogRoot>
</template>
