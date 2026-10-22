<script setup lang="ts">
/**
 * Server sign-in (P0 §1.3 + Phase 3 §3.3): pairing token, username/password
 * login, or the one-time first-admin setup — whichever the server needs.
 * Opens automatically on any 401 ("session expired — log in again" /
 * "server token changed"), or from Settings → Server.
 */
import { X } from "lucide-vue-next";
import { ref, watch } from "vue";
import { DialogClose, DialogContent, DialogDescription, DialogOverlay, DialogPortal, DialogRoot, DialogTitle } from "reka-ui";
import { ApiError, createFirstAdmin, setupNeeded } from "../api";
import { useLibraryStore } from "../stores/library";
import { useOverlaysStore } from "../stores/overlays";
import { usePlaylistsStore } from "../stores/playlists";
import { useSettingsStore } from "../stores/settings";
import StateMessage from "../ui/StateMessage.vue";
import UiButton from "../ui/UiButton.vue";
import UiInput from "../ui/UiInput.vue";

const overlays = useOverlaysStore();
const settings = useSettingsStore();
const lib = useLibraryStore();
const playlists = usePlaylistsStore();

type Mode = "login" | "pairing" | "setup";
const mode = ref<Mode>("login");
const setupRequired = ref<boolean | null>(null);

const username = ref("");
const password = ref("");
const pairingToken = ref("");
const busy = ref(false);
const error = ref<string | null>(null);

function reset(): void {
  username.value = "";
  password.value = "";
  pairingToken.value = "";
  error.value = null;
  setupRequired.value = null;
  mode.value = "login";
}

watch(
  () => overlays.authOpen,
  async (open) => {
    if (!open) return;
    reset();
    // A fresh server (no users yet) needs the one-time admin setup, not a login.
    try {
      setupRequired.value = await setupNeeded();
      mode.value = setupRequired.value ? "setup" : "login";
    } catch {
      setupRequired.value = false; // unreachable server: still offer the forms
    }
  },
);

async function submit(): Promise<void> {
  busy.value = true;
  error.value = null;
  try {
    if (mode.value === "setup") {
      if (!username.value.trim() || password.value.length < 8 || !pairingToken.value.trim()) {
        throw new Error("Username, a password of at least 8 characters, and the pairing token are required.");
      }
      await createFirstAdmin(username.value.trim(), password.value, pairingToken.value.trim());
      // The first admin retires the pairing token: sign in with the new account.
      await settings.login(username.value.trim(), password.value);
    } else if (mode.value === "login") {
      await settings.login(username.value.trim(), password.value);
    } else {
      await settings.saveServerToken(pairingToken.value);
    }
    overlays.authOpen = false;
    // Re-fetch the whole library against the (possibly new) credential.
    await Promise.all([lib.loadAll(), playlists.load()]);
  } catch (e) {
    if (e instanceof ApiError && e.status === 401) {
      error.value =
        mode.value === "pairing"
          ? "That token was rejected — check it against the server's pairing display."
          : "Invalid username or password.";
    } else if (e instanceof ApiError && e.status === 429) {
      error.value = "Too many attempts — wait a minute and try again.";
    } else {
      error.value = e instanceof Error ? e.message : String(e);
    }
  } finally {
    busy.value = false;
  }
}

const title = () => {
  if (overlays.authReason === "expired") return "Session expired";
  if (overlays.authReason === "token-changed") return "Server token changed";
  return "Sign in to the server";
};
</script>

<template>
  <DialogRoot v-model:open="overlays.authOpen">
    <DialogPortal>
      <DialogOverlay class="fixed inset-0 z-50 bg-black/55" data-kw-fade />
      <DialogContent
        data-kw-fade
        class="fixed left-1/2 top-1/2 z-50 flex max-h-[85vh] w-[calc(100%-2rem)] max-w-md -translate-x-1/2 -translate-y-1/2 flex-col rounded-lg border border-line bg-surface shadow-float outline-none"
        data-testid="auth-dialog"
      >
        <DialogTitle class="sr-only">{{ title() }}</DialogTitle>
        <DialogDescription class="sr-only">Sign in to the Kahawai server.</DialogDescription>
        <div class="flex items-center justify-between border-b border-line px-4 py-3">
          <h2 class="m-0 text-[15px] font-semibold">{{ title() }}</h2>
          <DialogClose as-child>
            <UiButton variant="icon" aria-label="Close"><X class="size-4" /></UiButton>
          </DialogClose>
        </div>

        <form class="flex flex-col gap-3 px-4 py-4" @submit.prevent="submit">
          <p v-if="overlays.authReason === 'expired'" class="m-0 text-[13px] text-dim">
            Your session expired — log in again.
          </p>
          <p v-if="overlays.authReason === 'token-changed'" class="m-0 text-[13px] text-dim">
            The server rejected the stored credential — it was regenerated or retired. Sign in again.
          </p>

          <div v-if="mode === 'setup'" class="flex flex-col gap-3">
            <p class="m-0 text-[13px] text-dim">
              This server has no user accounts yet. Create the first admin — the pairing token is
              retired afterwards.
            </p>
            <label class="flex flex-col gap-1 text-[13px]">
              Username
              <UiInput v-model="username" autocomplete="username" maxlength="64" />
            </label>
            <label class="flex flex-col gap-1 text-[13px]">
              Password (8+ characters)
              <UiInput v-model="password" type="password" autocomplete="new-password" />
            </label>
            <label class="flex flex-col gap-1 text-[13px]">
              Pairing token (shown once on the server)
              <UiInput v-model="pairingToken" type="password" autocomplete="off" />
            </label>
          </div>

          <div v-if="mode === 'login'" class="flex flex-col gap-3">
            <label class="flex flex-col gap-1 text-[13px]">
              Username
              <UiInput v-model="username" autocomplete="username" />
            </label>
            <label class="flex flex-col gap-1 text-[13px]">
              Password
              <UiInput v-model="password" type="password" autocomplete="current-password" />
            </label>
            <button type="button" class="self-start text-[13px] text-dim underline" @click="mode = 'pairing'">
              Use a pairing token instead
            </button>
          </div>

          <div v-if="mode === 'pairing'" class="flex flex-col gap-3">
            <p class="m-0 text-[13px] text-dim">
              Pair with the server using the token it shows once at startup (or in the server app).
            </p>
            <label class="flex flex-col gap-1 text-[13px]">
              Pairing token
              <UiInput v-model="pairingToken" type="password" autocomplete="off" />
            </label>
            <button type="button" class="self-start text-[13px] text-dim underline" @click="mode = 'login'">
              Use a username and password instead
            </button>
          </div>

          <StateMessage v-if="error" kind="error">{{ error }}</StateMessage>

          <div class="flex justify-end gap-2">
            <DialogClose as-child>
              <UiButton>Cancel</UiButton>
            </DialogClose>
            <UiButton variant="primary" type="submit" :disabled="busy" data-testid="auth-submit">
              {{ busy ? "Signing in…" : mode === "setup" ? "Create admin" : "Sign in" }}
            </UiButton>
          </div>
        </form>
      </DialogContent>
    </DialogPortal>
  </DialogRoot>
</template>
