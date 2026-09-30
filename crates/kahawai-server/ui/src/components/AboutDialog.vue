<script setup lang="ts">
/**
 * About Kahawai Server: the splash artwork, then the bundled Markdown
 * (src/content/about.md and the generated open-source notices; see
 * src/lib/about.ts). Same design as the player's About. Built on Reka UI's
 * Dialog (focus trap, Escape, aria-modal, focus return).
 */
import { X } from "lucide-vue-next";
import { computed, ref, watch } from "vue";
import { DialogClose, DialogContent, DialogDescription, DialogOverlay, DialogPortal, DialogRoot, DialogTitle } from "reka-ui";
import { licenseText, renderAbout, type AboutPage } from "../lib/about";
import { useSetupStore } from "../stores/setup";

const setup = useSetupStore();
const page = ref<AboutPage>("about");
// Injected by vite.config.ts; absent under some tooling.
const version = typeof __APP_VERSION__ === "string" ? __APP_VERSION__ : "dev";
const html = computed(() => (page.value === "license" ? "" : renderAbout(page.value, version)));

const tabs: { id: AboutPage; label: string }[] = [
  { id: "about", label: "About" },
  { id: "notices", label: "Open-source notices" },
  { id: "license", label: "License" },
];

// Always open on the About page, not wherever it was last left.
watch(
  () => setup.aboutOpen,
  (open) => {
    if (open) page.value = "about";
  },
);
</script>

<template>
  <DialogRoot v-model:open="setup.aboutOpen">
    <DialogPortal>
      <DialogOverlay class="fixed inset-0 z-50 bg-black/55" data-kw-fade />
      <DialogContent
        data-kw-fade
        class="fixed left-1/2 top-1/2 z-50 flex max-h-[85vh] w-[calc(100%-2rem)] max-w-2xl -translate-x-1/2 -translate-y-1/2 flex-col rounded-lg border border-line bg-surface shadow-float outline-none"
        data-testid="about-dialog"
      >
        <DialogTitle class="sr-only">About Kahawai Server</DialogTitle>
        <DialogDescription class="sr-only">Version, credits, copyright, the open-source notices and the license.</DialogDescription>
        <div class="flex items-center gap-1 border-b border-line px-4 py-2" role="tablist" aria-label="About pages">
          <button
            v-for="t in tabs"
            :key="t.id"
            type="button"
            role="tab"
            class="rounded-md border-0 px-2.5 py-1 text-[13px] transition-colors"
            :class="page === t.id ? 'bg-active text-fg' : 'bg-transparent text-dim hover:bg-hover hover:text-fg'"
            :aria-selected="page === t.id"
            :data-testid="`about-tab-${t.id}`"
            @click="page = t.id"
          >
            {{ t.label }}
          </button>
          <DialogClose
            class="ml-auto rounded-md border-0 bg-transparent px-1.5 py-1 text-dim hover:bg-hover hover:text-fg"
            title="Close (Esc)"
            aria-label="Close"
          >
            <X class="size-4" />
          </DialogClose>
        </div>
        <div class="overflow-y-auto px-6 py-5" role="tabpanel">
          <!-- The splash artwork, above the title (About page only). -->
          <img
            v-if="page === 'about'"
            src="/splash.webp"
            alt=""
            class="mb-5 block aspect-[3/2] w-full rounded-md object-cover"
            data-testid="about-image"
          />
          <p v-if="page === 'about' && setup.identity" class="m-0 mb-2 text-xs text-dim" data-testid="about-build">
            Build {{ setup.identity.build.commit }}{{ setup.identity.build.dirty ? "+changes" : "" }} ·
            {{ setup.identity.build.profile }} · {{ setup.identity.build.target }} · built {{ setup.identity.build.built_at }}
          </p>
          <!-- The GNU AGPL, exactly as in the repository's LICENSE. -->
          <pre
            v-if="page === 'license'"
            class="m-0 select-text whitespace-pre-wrap font-mono text-[11.5px] leading-relaxed text-dim"
            data-testid="about-license"
          >{{ licenseText }}</pre>
          <!-- Trusted content: bundled with the app, not user or network supplied. -->
          <div v-else class="markdown select-text" data-testid="about-content" v-html="html" />
        </div>
      </DialogContent>
    </DialogPortal>
  </DialogRoot>
</template>

<style scoped>
.markdown :deep(h1) {
  margin: 0 0 0.25rem;
  font-size: 24px;
  font-weight: 600;
  letter-spacing: -0.01em;
}
.markdown :deep(h2) {
  margin: 1.5rem 0 0.5rem;
  font-size: 11px;
  font-weight: 600;
  letter-spacing: 0.08em;
  text-transform: uppercase;
  color: var(--color-dim);
}
.markdown :deep(p),
.markdown :deep(li) {
  font-family: var(--font-serif);
  font-size: 15px;
  line-height: 1.6;
  color: var(--color-fg);
}
.markdown :deep(p) {
  margin: 0.5rem 0;
  max-width: 65ch;
}
.markdown :deep(ul) {
  margin: 0.5rem 0;
  padding-left: 1.25rem;
  list-style: disc;
}
.markdown :deep(li) {
  margin: 0.3rem 0;
}
.markdown :deep(strong) {
  font-weight: 600;
}
.markdown :deep(code) {
  font-family: var(--font-mono);
  font-size: 0.85em;
  color: var(--color-accent);
}
.markdown :deep(pre) {
  margin: 0.75rem 0;
  padding: 0.75rem;
  overflow-x: auto;
  border-radius: 4px;
  border: 1px solid var(--color-line);
  background: var(--color-canvas);
  font-size: 11px;
  line-height: 1.45;
}
.markdown :deep(pre code) {
  color: var(--color-dim);
  white-space: pre-wrap;
}
.markdown :deep(table) {
  width: 100%;
  margin: 0.5rem 0;
  border-collapse: collapse;
  font-size: 12.5px;
}
.markdown :deep(th),
.markdown :deep(td) {
  padding: 0.3rem 0.5rem;
  border-bottom: 1px solid var(--color-line);
  text-align: left;
  vertical-align: top;
  color: var(--color-fg);
}
.markdown :deep(th) {
  font-weight: 600;
  color: var(--color-dim);
}
</style>
