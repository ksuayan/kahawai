<script setup lang="ts">
import { onMounted } from "vue";
import StatusView from "./components/StatusView.vue";
import WizardView from "./components/WizardView.vue";
import { useSetupStore } from "./stores/setup";
import { setupAppReady, setupServerStatus } from "./tauri";

const setup = useSetupStore();

/** How long the splash waits for the server to come up at launch. */
const SERVER_START_WAIT_MS = 6000;

onMounted(async () => {
  await setup.init();
  // The splash stays up while the server starts (autostart runs alongside
  // this), so the window opens on "Server running", not a passing "not
  // running". The wizard (no config yet) has no server to wait for.
  if (setup.view === "status" && !setup.serverStatus?.running) {
    const until = Date.now() + SERVER_START_WAIT_MS;
    while (Date.now() < until) {
      await new Promise((r) => setTimeout(r, 300));
      const status = await setupServerStatus();
      if (status?.running) {
        setup.serverStatus = status;
        break;
      }
    }
  }
  await setupAppReady();
});
</script>

<template>
  <div class="flex h-full flex-col">
    <div v-if="setup.loading" class="flex h-full items-center justify-center text-faint">Starting…</div>
    <WizardView v-else-if="setup.view === 'wizard'" />
    <StatusView v-else />
  </div>
</template>
