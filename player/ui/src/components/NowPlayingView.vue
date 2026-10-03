<script setup lang="ts">
import { computed } from "vue";
import { useAudiobooksStore } from "../stores/audiobooks";
import { usePodcastsStore } from "../stores/podcasts";
import ViewShell from "../ui/ViewShell.vue";
import NowPlayingContent from "./NowPlayingContent.vue";

const books = useAudiobooksStore();
const podcasts = usePodcastsStore();
/** The breadcrumb section follows the content: a book reads as Audiobooks, an episode as Podcasts. */
const section = computed(() => (books.isActive ? "audiobooks" : podcasts.isActive ? "podcasts" : "albums") as "audiobooks" | "podcasts" | "albums");
</script>

<template>
  <ViewShell width="medium" :section="section" crumb="Now Playing">
    <NowPlayingContent />
  </ViewShell>
</template>
