<script setup lang="ts">
import { ExternalLink, Play, RefreshCw, Settings, Trash2 } from "lucide-vue-next";
import { ContextMenuContent, ContextMenuItem, ContextMenuPortal, ContextMenuRoot, ContextMenuSeparator, ContextMenuTrigger } from "reka-ui";
import { usePodcastsStore } from "../stores/podcasts";
import { openUrl } from "../tauri";
import type { PodcastFeed } from "../types";

/**
 * Right-click menu for a show's tile or row: Play (its newest unplayed
 * episode), Check for new, Settings, Website, Delete. Wraps its single child,
 * which becomes the trigger. Settings and Delete are the view's to show (one
 * dialog per view, not per tile).
 */
const props = defineProps<{ feed: PodcastFeed }>();
const emit = defineEmits<{ (e: "settings", feed: PodcastFeed): void; (e: "delete", feed: PodcastFeed): void }>();
const podcasts = usePodcastsStore();

const itemClass =
  "flex cursor-default select-none items-center justify-between gap-3 whitespace-nowrap rounded-md px-2.5 py-2 text-[13px] text-fg outline-none " +
  "data-[disabled]:opacity-40 data-[highlighted]:bg-hover";
const contentClass = "z-[60] min-w-[190px] rounded-md border border-line bg-surface p-1 shadow-float";
</script>

<template>
  <ContextMenuRoot>
    <ContextMenuTrigger as-child>
      <slot />
    </ContextMenuTrigger>
    <ContextMenuPortal>
      <ContextMenuContent :class="contentClass" data-kw-fade data-testid="podcast-menu">
        <ContextMenuItem :class="itemClass" :disabled="props.feed.unplayed_count === 0" data-testid="podcast-menu-play" @select="podcasts.playLatest(props.feed)">
          <span class="flex items-center gap-2"><Play class="size-4 text-dim" />Play</span>
        </ContextMenuItem>
        <ContextMenuItem :class="itemClass" data-testid="podcast-menu-refresh" @select="podcasts.refresh(props.feed.id)">
          <span class="flex items-center gap-2"><RefreshCw class="size-4 text-dim" />Check for new</span>
        </ContextMenuItem>
        <ContextMenuItem :class="itemClass" data-testid="podcast-menu-settings" @select="emit('settings', props.feed)">
          <span class="flex items-center gap-2"><Settings class="size-4 text-dim" />Settings</span>
        </ContextMenuItem>
        <ContextMenuItem :class="itemClass" :disabled="!props.feed.link" data-testid="podcast-menu-website" @select="props.feed.link && openUrl(props.feed.link)">
          <span class="flex items-center gap-2"><ExternalLink class="size-4 text-dim" />Website</span>
        </ContextMenuItem>
        <ContextMenuSeparator class="my-1 h-px bg-line" />
        <ContextMenuItem :class="itemClass" data-testid="podcast-menu-delete" @select="emit('delete', props.feed)">
          <span class="flex items-center gap-2 text-danger-fg"><Trash2 class="size-4" />Delete</span>
        </ContextMenuItem>
      </ContextMenuContent>
    </ContextMenuPortal>
  </ContextMenuRoot>
</template>
