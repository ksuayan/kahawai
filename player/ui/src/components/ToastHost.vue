<script setup lang="ts">
import { useToastsStore } from "../stores/toasts";

const toasts = useToastsStore();
</script>

<template>
  <div class="toast-host" aria-live="polite">
    <div v-for="t in toasts.toasts" :key="t.id" class="toast" :class="`kind-${t.kind}`">
      <div class="t-body">
        <div class="t-title">{{ t.title }}</div>
        <div v-if="t.detail" class="t-detail">{{ t.detail }}</div>
        <div v-if="t.progress != null" class="t-bar">
          <div class="t-fill" :style="{ width: `${Math.round(t.progress * 100)}%` }" />
        </div>
      </div>
      <button class="icon-btn t-close" title="Dismiss" @click="toasts.dismiss(t.id)">✕</button>
    </div>
  </div>
</template>

<style scoped>
.toast-host {
  position: fixed;
  right: 16px;
  bottom: 96px;
  display: flex;
  flex-direction: column;
  gap: 8px;
  z-index: 100;
  max-width: 360px;
}

.toast {
  display: flex;
  align-items: flex-start;
  gap: 8px;
  background: var(--bg-raised);
  border: 1px solid var(--border);
  border-left-width: 3px;
  border-radius: 10px;
  padding: 10px 12px;
  box-shadow: 0 8px 24px rgba(0, 0, 0, 0.45);
  font-size: 13px;
}

.kind-info {
  border-left-color: #0a84ff;
}
.kind-success {
  border-left-color: #30d158;
}
.kind-error {
  border-left-color: #ff453a;
}
.kind-progress {
  border-left-color: #ffd60a;
}

.t-body {
  flex: 1;
  min-width: 0;
}

.t-title {
  font-weight: 600;
}

.t-detail {
  color: var(--text-dim);
  font-size: 12px;
  margin-top: 2px;
  white-space: pre-wrap;
  word-break: break-word;
}

.t-bar {
  height: 4px;
  background: var(--bg-active);
  border-radius: 2px;
  margin-top: 8px;
  overflow: hidden;
}

.t-fill {
  height: 100%;
  background: #ffd60a;
  transition: width 0.4s linear;
}

.t-close {
  font-size: 11px;
  padding: 2px 6px;
}
</style>
