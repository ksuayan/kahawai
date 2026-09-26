<script setup lang="ts">
import { X } from "lucide-vue-next";
import {
  ToastClose,
  ToastDescription,
  ToastProvider,
  ToastRoot,
  ToastTitle,
  ToastViewport,
} from "reka-ui";
import { useToastsStore, type ToastKind } from "../stores/toasts";

const toasts = useToastsStore();

const accent: Record<ToastKind, string> = {
  info: "border-l-accent",
  success: "border-l-ok",
  error: "border-l-danger",
  progress: "border-l-warn",
};
</script>

<template>
  <!-- The store owns timing (ttl / sticky progress); Reka provides the
       accessible live region, swipe-to-dismiss and keyboard handling. -->
  <ToastProvider :duration="Infinity" swipe-direction="right" label="Notification">
    <ToastRoot
      v-for="t in toasts.toasts"
      :key="t.id"
      :open="true"
      :duration="Infinity"
      :type="t.kind === 'error' ? 'foreground' : 'background'"
      :data-kind="t.kind"
      class="flex items-start gap-2 rounded-lg border border-l-[3px] border-line bg-surface px-3 py-2.5 text-[13px] shadow-float"
      :class="accent[t.kind]"
      @update:open="(open: boolean) => !open && toasts.dismiss(t.id)"
    >
      <div class="min-w-0 flex-1">
        <ToastTitle class="font-semibold">{{ t.title }}</ToastTitle>
        <ToastDescription v-if="t.detail" class="mt-0.5 whitespace-pre-wrap break-words text-xs text-dim">
          {{ t.detail }}
        </ToastDescription>
        <div v-if="t.progress != null" class="mt-2 h-1 overflow-hidden rounded-sm bg-active">
          <div
            class="h-full bg-warn transition-[width] duration-[400ms] ease-linear"
            :style="{ width: `${Math.round(t.progress * 100)}%` }"
            role="progressbar"
            :aria-valuenow="Math.round(t.progress * 100)"
            aria-valuemin="0"
            aria-valuemax="100"
          />
        </div>
      </div>
      <ToastClose
        aria-label="Dismiss"
        title="Dismiss"
        class="rounded-md px-1.5 py-0.5 text-[11px] text-dim hover:bg-hover hover:text-fg"
      >
        <X class="size-3.5" />
      </ToastClose>
    </ToastRoot>
    <ToastViewport
      class="fixed bottom-24 right-4 z-[100] m-0 flex max-w-[360px] list-none flex-col gap-2 p-0 outline-none"
    />
  </ToastProvider>
</template>
