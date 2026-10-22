<script setup lang="ts">
import { computed, ref } from "vue";
import UiHint from "../ui/UiHint.vue";
import UiSelect, { type UiSelectOption } from "../ui/UiSelect.vue";
import { useSetupStore } from "../stores/setup";

const setup = useSetupStore();
const advancedOpen = ref(false);

// SEC-1 + Phase 2: who may reach the server, not a raw address.
const scopeOptions: UiSelectOption[] = [
  { value: "thisMac", label: "This Mac only" },
  { value: "localNetwork", label: "Local network" },
  { value: "tailscaleOnly", label: "Tailscale only" },
];
const scopeValue = computed({
  get: () => setup.listenScope,
  set: (v: string | null) => {
    setup.listenScope = v === "tailscaleOnly" ? "tailscaleOnly" : v === "localNetwork" ? "localNetwork" : "thisMac";
  },
});
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
        <dt class="micro-label text-faint">Audiobook folders</dt>
        <dd class="mt-0.5" data-testid="review-audiobooks">
          <div v-for="d in setup.audiobookDirs" :key="d.path">{{ d.path }}</div>
          <span v-if="setup.audiobookDirs.length === 0" class="text-faint">None</span>
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
      <label class="micro-label mb-1 block text-faint" for="listen-scope">Who can reach this server</label>
      <UiSelect id="listen-scope" v-model="scopeValue" :options="scopeOptions" />
      <UiHint v-if="setup.listenScope === 'localNetwork'" tone="warn" data-testid="trusted-network-warning">
        This exposes the server to every device on your network — only do this on a
        network you fully trust (your home LAN, not a coffee shop or office guest Wi-Fi).
        API access still requires the pairing token.
      </UiHint>
      <UiHint v-if="setup.listenScope === 'tailscaleOnly' && setup.tailscaleStatus" tone="dim">
        Binds {{ setup.tailscaleStatus.ipv4
        }}<template v-if="setup.tailscaleStatus.magic_dns"> ({{ setup.tailscaleStatus.magic_dns }})</template>
        — reachable from your tailnet only.
      </UiHint>
      <UiHint v-if="setup.listenScope === 'tailscaleOnly' && !setup.tailscaleStatus" tone="warn">
        Tailscale isn't running on this Mac — the server will refuse to start
        until it is. Start Tailscale and reopen this step.
      </UiHint>
    </div>

    <UiHint v-if="setup.saveError" tone="warn">{{ setup.saveError }}</UiHint>
  </div>
</template>
