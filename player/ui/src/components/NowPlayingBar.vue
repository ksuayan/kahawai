<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";
import { useLibraryStore } from "../stores/library";
import { useNavStore } from "../stores/nav";
import { usePlayerStore } from "../stores/player";
import { useSettingsStore } from "../stores/settings";
import {
  STREAM_FORMATS,
  formatDuration,
  isPlayable,
  trackTitle,
  unplayableReason,
  validFormatsFor,
  type StreamFormat,
} from "../types";
import Artwork from "./Artwork.vue";
import TrackMenu from "./TrackMenu.vue";

const player = usePlayerStore();
const settings = useSettingsStore();
const lib = useLibraryStore();
const nav = useNavStore();

// --- seek bar ---
const scrubbing = ref(false);
const scrubValue = ref(0);

const duration = computed(() => player.durationMs ?? 0);
const displayPos = computed(() => (scrubbing.value ? scrubValue.value : player.positionMs));

function onScrub(e: Event): void {
  scrubbing.value = true;
  scrubValue.value = Number((e.target as HTMLInputElement).value);
}

function commitScrub(e: Event): void {
  const v = Number((e.target as HTMLInputElement).value);
  scrubValue.value = v;
  scrubbing.value = false;
  void player.seekTo(v);
}

// --- volume ---
const volumePct = ref(100);
onMounted(() => {
  volumePct.value = Math.round(player.volume * 100);
});
watch(
  () => player.volume,
  (v) => {
    volumePct.value = Math.round(v * 100);
  },
);

function onVolume(e: Event): void {
  const v = Number((e.target as HTMLInputElement).value) / 100;
  volumePct.value = Number((e.target as HTMLInputElement).value);
  void player.changeVolume(v);
}

// --- format picker ---
const track = computed(() => player.currentTrack);
const options = computed(() => (track.value ? validFormatsFor(track.value) : []));
const trackPlayable = computed(() => (track.value ? isPlayable(track.value) : false));

function onTrackFormat(e: Event): void {
  if (!track.value) return;
  const v = (e.target as HTMLSelectElement).value;
  void player.changeTrackFormat(track.value.id, v === "" ? null : (v as StreamFormat));
}

// --- settings popover (global format) ---
const popoverOpen = ref(false);

function onGlobalFormat(e: Event): void {
  const v = (e.target as HTMLSelectElement).value;
  void settings.saveGlobalFormat(v === "" ? null : (v as StreamFormat));
}

function togglePopover(): void {
  popoverOpen.value = !popoverOpen.value;
}

const artworkHash = computed(() => {
  const t = track.value;
  if (!t?.album_id) return null;
  return lib.albums.find((a) => a.id === t.album_id)?.artwork_hash ?? null;
});

function goQueue(): void {
  nav.go("queue");
}

function goNowPlaying(): void {
  if (track.value) nav.go("nowplaying");
}

function repeatTitle(): string {
  return player.repeat === "off"
    ? "Repeat off"
    : player.repeat === "all"
      ? "Repeat all"
      : "Repeat one";
}
</script>

<template>
  <footer class="now-playing">
    <div v-if="player.error" class="player-error" :title="player.error">
      ⚠ {{ player.error }}
    </div>
    <div class="bar">
      <!-- left: identity (click → full now-playing view) -->
      <button class="identity link" title="Open full now-playing view" @click="goNowPlaying">
        <Artwork :hash="artworkHash" :size="44" :radius="6" :alt="track ? trackTitle(track) : 'No track'" />
        <div class="titles">
          <div class="title">{{ track ? trackTitle(track) : "Nothing playing" }}</div>
          <div class="artist">{{ track?.artist ?? "—" }}</div>
        </div>
        <span v-if="player.chain" class="badge accent chain" :title="`Audio chain: ${player.chain}`">
          {{ player.chain }}
        </span>
        <span
          v-if="player.isDopExclusive"
          class="badge dop"
          title="Exclusive DoP output: bit-perfect, bypasses EQ, loudness, and volume"
        >
          Exclusive DoP
        </span>
      </button>

      <!-- center: transport + seek -->
      <div class="transport">
        <div class="buttons">
          <button
            class="icon-btn tbtn"
            :class="{ on: player.shuffle }"
            title="Shuffle"
            :disabled="!track"
            @click="player.toggleShuffle()"
          >
            🔀
          </button>
          <button
            class="icon-btn tbtn"
            title="Previous (P)"
            :disabled="!track"
            @click="player.prevTrack()"
          >
            ⏮
          </button>
          <button
            class="icon-btn tbtn main"
            :title="player.isPlaying ? 'Pause (Space)' : 'Play (Space)'"
            :disabled="!track"
            @click="player.toggle()"
          >
            {{ player.isLoading ? "…" : player.isPlaying ? "⏸" : "▶" }}
          </button>
          <button
            class="icon-btn tbtn"
            title="Next (N)"
            :disabled="!track"
            @click="player.nextTrack()"
          >
            ⏭
          </button>
          <button class="icon-btn tbtn" title="Stop" :disabled="!track" @click="player.stop()">
            ⏹
          </button>
          <button
            class="icon-btn tbtn"
            :class="{ on: player.repeat !== 'off' }"
            :title="repeatTitle()"
            :disabled="!track"
            @click="player.cycleRepeat()"
          >
            {{ player.repeat === "one" ? "🔂" : "🔁" }}
          </button>
        </div>
        <div class="seek-row">
          <span class="time">{{ formatDuration(displayPos) }}</span>
          <input
            type="range"
            class="seek"
            :min="0"
            :max="Math.max(duration, 1)"
            :value="Math.min(displayPos, Math.max(duration, 1))"
            :disabled="!track || duration <= 0"
            @input="onScrub"
            @change="commitScrub"
          />
          <span class="time">{{ formatDuration(duration || null) }}</span>
        </div>
      </div>

      <!-- right: format, volume, extras -->
      <div class="controls">
        <select
          class="format-picker"
          :value="player.activeFormat ?? ''"
          :disabled="!track || !trackPlayable"
          :title="
            !track
              ? 'No track playing'
              : !trackPlayable
                ? unplayableReason(track)
                : 'Stream format for this track'
          "
          @change="onTrackFormat"
        >
          <option value="">Auto</option>
          <option v-for="f in options" :key="f" :value="f">{{ f.toUpperCase() }}</option>
        </select>
        <input
          type="range"
          class="volume"
          min="0"
          max="100"
          :value="volumePct"
          :class="{ ignored: player.isDopExclusive }"
          :title="player.isDopExclusive ? 'Volume is ignored on exclusive DoP output' : 'Volume'"
          @input="onVolume"
        />
        <button class="icon-btn" title="Queue" @click="goQueue">☰</button>
        <TrackMenu v-if="track" :track="track" />
        <div class="popover-wrap">
          <button class="icon-btn" title="Playback settings" @click="togglePopover">⚙</button>
          <div v-if="popoverOpen" class="popover">
            <label>Default format</label>
            <select :value="settings.globalFormat ?? ''" @change="onGlobalFormat">
              <option value="">Auto (server default)</option>
              <option v-for="f in STREAM_FORMATS" :key="f" :value="f">
                {{ f === "passthrough" ? "Passthrough" : f.toUpperCase() }}
              </option>
            </select>
            <p class="hint">Applies to tracks without a per-track override.</p>
          </div>
        </div>
      </div>
    </div>
  </footer>
</template>

<style scoped>
.now-playing {
  border-top: 1px solid var(--border);
  background: var(--bg-raised);
  position: relative;
  z-index: 20;
}

.player-error {
  background: rgba(255, 69, 58, 0.14);
  color: #ff9d97;
  font-size: 12px;
  padding: 6px 16px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.bar {
  display: grid;
  grid-template-columns: 1fr 1.4fr 1fr;
  align-items: center;
  gap: 16px;
  padding: 8px 16px;
  min-height: 68px;
}

.identity {
  display: flex;
  align-items: center;
  gap: 10px;
  min-width: 0;
}

.identity.link {
  background: transparent;
  border: none;
  text-align: left;
  cursor: pointer;
  padding: 0;
  color: inherit;
}

.identity.link:hover .title {
  color: #0a84ff;
}

.tbtn.on {
  color: #0a84ff;
  background: rgba(10, 132, 255, 0.14);
}

.titles {
  min-width: 0;
}

.title {
  font-weight: 600;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.artist {
  color: var(--text-dim);
  font-size: 12px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.chain {
  margin-left: 4px;
}

.badge.dop {
  margin-left: 4px;
  background: rgba(48, 209, 88, 0.16);
  color: #30d158;
  border: 1px solid rgba(48, 209, 88, 0.4);
}

.volume.ignored {
  opacity: 0.4;
}

.transport {
  display: flex;
  flex-direction: column;
  align-items: stretch;
  gap: 2px;
}

.buttons {
  display: flex;
  justify-content: center;
  gap: 4px;
}

.tbtn {
  font-size: 15px;
  padding: 4px 10px;
}

.tbtn.main {
  font-size: 17px;
  color: var(--text);
}

.seek-row {
  display: flex;
  align-items: center;
  gap: 8px;
}

.time {
  font-size: 11px;
  color: var(--text-dim);
  font-variant-numeric: tabular-nums;
  min-width: 44px;
  text-align: center;
}

.seek {
  flex: 1;
}

.controls {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 10px;
}

.format-picker {
  max-width: 130px;
}

.volume {
  width: 100px;
}

.popover-wrap {
  position: relative;
}

.popover {
  position: absolute;
  bottom: 40px;
  right: 0;
  width: 230px;
  background: var(--bg-hover);
  border: 1px solid var(--border);
  border-radius: 10px;
  padding: 12px;
  box-shadow: 0 8px 24px rgba(0, 0, 0, 0.5);
}

.popover label {
  display: block;
  font-size: 12px;
  color: var(--text-dim);
  margin-bottom: 6px;
}

.popover select {
  width: 100%;
}

.popover .hint {
  font-size: 11px;
  color: var(--text-faint);
  margin: 8px 0 0;
}
</style>
