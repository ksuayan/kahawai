<script setup lang="ts">
import {
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogOverlay,
  DialogPortal,
  DialogRoot,
  DialogTitle,
} from "reka-ui";

/** Modal dialog (Reka Dialog): focus trap, Escape, scroll lock, aria wiring. */
defineProps<{ open: boolean; title: string; description?: string; wide?: boolean }>();
const emit = defineEmits<{ (e: "update:open", v: boolean): void }>();
</script>

<template>
  <DialogRoot :open="open" @update:open="(v) => emit('update:open', v)">
    <DialogPortal>
      <DialogOverlay class="fixed inset-0 z-50 bg-black/55" />
      <DialogContent
        class="fixed left-1/2 top-1/2 z-50 max-h-[calc(100vh-32px)] -translate-x-1/2 -translate-y-1/2 overflow-y-auto rounded-xl border border-line bg-raised p-5 shadow-[0_16px_48px_rgba(0,0,0,0.55)] outline-none"
        :class="wide ? 'w-[min(820px,calc(100vw-32px))]' : 'w-[min(440px,calc(100vw-32px))]'"
      >
        <DialogTitle class="m-0 text-[15px] font-semibold">{{ title }}</DialogTitle>
        <DialogDescription v-if="description" class="mt-1 text-[12px] text-dim">
          {{ description }}
        </DialogDescription>
        <div class="mt-4"><slot /></div>
        <div v-if="$slots.footer" class="mt-5 flex justify-end gap-2"><slot name="footer" /></div>
        <DialogClose class="sr-only">Close</DialogClose>
      </DialogContent>
    </DialogPortal>
  </DialogRoot>
</template>
