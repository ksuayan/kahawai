<script setup lang="ts">
import { ref } from "vue";
import UiHint from "../ui/UiHint.vue";
import UiInput from "../ui/UiInput.vue";
import { useSetupStore } from "../stores/setup";

const setup = useSetupStore();
const advancedOpen = ref(false);
</script>

<template>
  <div>
    <h2 class="heading-1 mb-2">Review</h2>

    <dl class="mb-4 flex flex-col gap-2 text-[13px]">
      <div>
        <dt class="micro-label text-faint">Music folders</dt>
        <dd class="mt-0.5">
          <div v-for="d in setup.dirs" :key="d.path">{{ d.path }}</div>
        </dd>
      </div>
      <div>
        <dt class="micro-label text-faint">Database</dt>
        <dd class="mt-0.5">{{ setup.dbDir }}/music.db</dd>
      </div>
      <div>
        <dt class="micro-label text-faint">Configuration file</dt>
        <dd class="mt-0.5">{{ setup.configPath }}</dd>
      </div>
    </dl>

    <button type="button" class="mb-2 text-[13px] text-dim hover:text-fg" @click="advancedOpen = !advancedOpen">
      {{ advancedOpen ? "Hide" : "Show" }} advanced options
    </button>
    <div v-if="advancedOpen" class="mb-2">
      <label class="micro-label mb-1 block text-faint" for="bind-address">Bind address</label>
      <UiInput id="bind-address" v-model="setup.bind" placeholder="0.0.0.0:8080" />
      <UiHint v-if="!setup.bindLooksValid" tone="warn">Enter a host:port address, e.g. 0.0.0.0:8080.</UiHint>
    </div>

    <UiHint v-if="setup.saveError" tone="warn">{{ setup.saveError }}</UiHint>
  </div>
</template>
