<script setup lang="ts">
defineProps<{ title: string }>();
</script>

<template>
  <section class="mb-7">
    <h3 class="heading-3 m-0 mb-1 flex items-center gap-2">
      {{ title }}
      <slot name="aside" />
    </h3>
    <slot />
  </section>
</template>
