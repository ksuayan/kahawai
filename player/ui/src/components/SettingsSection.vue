<script setup lang="ts">
defineProps<{ title: string }>();
</script>

<template>
  <section class="mb-7">
    <h3 class="m-0 mb-1 text-sm font-semibold">{{ title }}</h3>
    <slot />
  </section>
</template>
