<script setup lang="ts">
import { Info, Pencil, Play } from "lucide-vue-next";
import { ContextMenuContent, ContextMenuItem, ContextMenuPortal, ContextMenuRoot, ContextMenuTrigger } from "reka-ui";
import { useAudiobooksStore } from "../stores/audiobooks";
import type { Audiobook } from "../types";

/**
 * Right-click menu for a book's card or list row: Play (from where you left
 * off), Info and Edit details. Wraps its single child, which becomes the
 * trigger. The dialogs live in BookDialogs, once per view.
 */
const props = defineProps<{ book: Audiobook }>();
const books = useAudiobooksStore();

const itemClass =
  "flex cursor-default select-none items-center justify-between gap-3 whitespace-nowrap rounded-md px-2.5 py-2 text-[13px] text-fg outline-none " +
  "data-[disabled]:opacity-40 data-[highlighted]:bg-hover";
const contentClass = "z-[60] min-w-[180px] rounded-md border border-line bg-surface p-1 shadow-float";
</script>

<template>
  <ContextMenuRoot>
    <ContextMenuTrigger as-child>
      <slot />
    </ContextMenuTrigger>
    <ContextMenuPortal>
      <ContextMenuContent :class="contentClass" data-kw-fade data-testid="book-menu">
        <ContextMenuItem :class="itemClass" data-testid="book-menu-play" @select="books.start(props.book.id)">
          <span class="flex items-center gap-2"><Play class="size-4 text-dim" />Play</span>
        </ContextMenuItem>
        <ContextMenuItem :class="itemClass" data-testid="book-menu-info" @select="books.showBookDialog('info', props.book.id)">
          <span class="flex items-center gap-2"><Info class="size-4 text-dim" />Info</span>
        </ContextMenuItem>
        <ContextMenuItem :class="itemClass" data-testid="book-menu-edit" @select="books.showBookDialog('edit', props.book.id)">
          <span class="flex items-center gap-2"><Pencil class="size-4 text-dim" />Edit details</span>
        </ContextMenuItem>
      </ContextMenuContent>
    </ContextMenuPortal>
  </ContextMenuRoot>
</template>
