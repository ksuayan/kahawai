<script setup lang="ts">
import { computed } from "vue";
import { useAudiobooksStore } from "../stores/audiobooks";
import ViewShell from "../ui/ViewShell.vue";
import NowPlayingContent from "./NowPlayingContent.vue";

const books = useAudiobooksStore();
/** The breadcrumb section follows the content: a book reads as Audiobooks. */
const section = computed(() => (books.isActive ? "audiobooks" : "albums") as "audiobooks" | "albums");
</script>

<template>
  <ViewShell width="medium" :section="section" crumb="Now Playing">
    <NowPlayingContent />
  </ViewShell>
</template>
