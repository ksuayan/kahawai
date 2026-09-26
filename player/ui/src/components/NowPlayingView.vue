<script setup lang="ts">
import { computed, ref } from "vue";
import { artworkSrc } from "../api";
import { useDspStore } from "../stores/dsp";
import { useLibraryStore } from "../stores/library";
import { useNavStore } from "../stores/nav";
import { usePlayerStore } from "../stores/player";
import {
  STREAM_FORMATS,
  formatBadge,
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
const lib = useLibraryStore();
const nav = useNavStore();
const dsp = useDspStore();

const track = computed(() => player.currentTrack);

// --- seek ---
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
const volumePct = computed({
  get: () => Math.round(player.volume * 100),
  set: (v: number) => void player.changeVolume(v / 100),
});

// --- composed audio-path badge ---
/** e.g. "DSF DSD64 → DoP → Exclusive (bit-perfect)" or
 *  "FLAC 44.1k/16 → FLAC transcode → PCM shared · EQ on". */
const audioPath = computed(() => {
  const t = track.value;
  if (!t) return "Nothing playing";
  const src = formatBadge(t);
  const stream = player.activeFormat ? player.activeFormat.toUpperCase() : "AUTO";
  const out = player.isDopExclusive ? "Exclusive DoP · bit-perfect" : "PCM shared";
  const dspBits: string[] = [];
  if (!player.isDopExclusive) {
    if (dsp.eqEnabled && dsp.activeBands.length > 0) dspBits.push(`EQ ${dsp.activeBands.length} bands`);
    if (dsp.loudnessEnabled) dspBits.push(`Loudness ${dsp.loudnessTarget} LUFS`);
  }
  return `${src} → ${stream} → ${out}${dspBits.length ? ` · ${dspBits.join(" · ")}` : ""}`;
});

const artworkHash = computed(() => {
  const t = track.value;
  if (!t?.album_id) return null;
  return lib.albums.find((a) => a.id === t.album_id)?.artwork_hash ?? null;
});

const artworkUrl = computed(() => artworkSrc(artworkHash.value));

function onTrackFormat(e: Event): void {
  if (!track.value) return;
  const v = (e.target as HTMLSelectElement).value;
  void player.changeTrackFormat(track.value.id, v === "" ? null : (v as StreamFormat));
}

function goEq(): void {
  nav.go("settings");
}

function repeatLabel(): string {
  return player.repeat === "off" ? "Repeat off" : player.repeat === "all" ? "Repeat all" : "Repeat one";
}
</script>

<template>
  <div class="view now-playing-view">
    <button class="back icon-btn" @click="nav.go('albums')">‹ Library</button>
    <div v-if="!track" class="empty">
      <p>Nothing playing.</p>
      <p class="sub">Pick an album or playlist to start.</p>
    </div>
    <div v-else class="np">
      <div class="art">
        <Artwork :hash="artworkHash" :size="320" :radius="12" :alt="trackTitle(track)" />
      </div>
      <div class="info">
        <h2>{{ trackTitle(track) }}</h2>
        <p class="artist">{{ track.artist ?? "Unknown artist" }}</p>
        <p class="album">{{ track.album ?? "" }}</p>

        <div class="badges">
          <span class="badge">{{ formatBadge(track) }}</span>
          <span class="badge accent" :title="`Audio chain: ${player.chain ?? '—'}`">
            {{ audioPath }}
          </span>
          <span
            v-if="player.isDopExclusive"
            class="badge dop"
            title="Exclusive DoP output: bit-perfect, bypasses EQ, loudness, and volume"
          >
            Exclusive DoP
          </span>
          <span v-if="!isPlayable(track)" class="badge danger">
            {{ unplayableReason(track) }}
          </span>
        </div>

        <!-- seek -->
        <div class="seek-row">
          <span class="time">{{ formatDuration(displayPos) }}</span>
          <input
            type="range"
            class="seek"
            :min="0"
            :max="Math.max(duration, 1)"
            :value="Math.min(displayPos, Math.max(duration, 1))"
            :disabled="duration <= 0"
            @input="onScrub"
            @change="commitScrub"
          />
          <span class="time">{{ formatDuration(duration || null) }}</span>
        </div>
        <p class="buffer-note">
          Buffer: not exposed by the engine — the stream is progressive HTTP
          (the position above is the playhead).
        </p>

        <!-- transport -->
        <div class="transport">
          <button
            class="icon-btn tbtn"
            :class="{ on: player.shuffle }"
            title="Shuffle"
            @click="player.toggleShuffle()"
          >
            🔀
          </button>
          <button class="icon-btn tbtn" title="Previous (P)" @click="player.prevTrack()">⏮</button>
          <button
            class="icon-btn tbtn main"
            :title="player.isPlaying ? 'Pause (Space)' : 'Play (Space)'"
            @click="player.toggle()"
          >
            {{ player.isLoading ? "…" : player.isPlaying ? "⏸" : "▶" }}
          </button>
          <button class="icon-btn tbtn" title="Next (N)" @click="player.nextTrack()">⏭</button>
          <button
            class="icon-btn tbtn"
            :class="{ on: player.repeat !== 'off' }"
            :title="repeatLabel()"
            @click="player.cycleRepeat()"
          >
            {{ player.repeat === "one" ? "🔂" : "🔁" }}
          </button>
        </div>

        <!-- extras -->
        <div class="extras">
          <label class="vol">
            Volume
            <input
              type="range"
              min="0"
              max="100"
              v-model.number="volumePct"
              :class="{ ignored: player.isDopExclusive }"
              :title="player.isDopExclusive ? 'Volume is ignored on exclusive DoP output' : 'Volume'"
            />
          </label>
          <label class="fmt">
            Track format
            <select
              :value="player.activeFormat ?? ''"
              :disabled="!isPlayable(track)"
              @change="onTrackFormat"
            >
              <option value="">Auto</option>
              <option v-for="f in validFormatsFor(track)" :key="f" :value="f">
                {{ f.toUpperCase() }}
              </option>
            </select>
          </label>
          <button class="icon-btn" title="Open EQ in Settings" @click="goEq">🎚 EQ</button>
          <TrackMenu :track="track" />
        </div>

        <p v-if="player.error" class="error-banner">{{ player.error }}</p>
        <p v-if="artworkUrl" class="sub art-note">
          Artwork loads through the browser HTTP cache (no dedicated disk cache in v1).
        </p>
      </div>
    </div>
  </div>
</template>

<style scoped>
.now-playing-view {
  max-width: 900px;
}

.back {
  margin-bottom: 16px;
}

.np {
  display: flex;
  gap: 32px;
  align-items: flex-start;
}

.art {
  flex-shrink: 0;
}

.info {
  flex: 1;
  min-width: 0;
}

.info h2 {
  margin: 0 0 4px;
  font-size: 24px;
}

.artist {
  font-size: 16px;
  color: var(--text-dim);
  margin: 0 0 2px;
}

.album {
  font-size: 14px;
  color: var(--text-faint);
  margin: 0 0 16px;
}

.badges {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  margin-bottom: 20px;
}

.badge.dop {
  background: rgba(48, 209, 88, 0.16);
  color: #30d158;
  border: 1px solid rgba(48, 209, 88, 0.4);
}

.badge.danger {
  background: rgba(255, 69, 58, 0.14);
  color: #ff9d97;
  border: 1px solid rgba(255, 69, 58, 0.4);
}

.seek-row {
  display: flex;
  align-items: center;
  gap: 12px;
  margin-bottom: 4px;
}

.time {
  font-size: 12px;
  color: var(--text-dim);
  font-variant-numeric: tabular-nums;
  min-width: 48px;
  text-align: center;
}

.seek {
  flex: 1;
}

.buffer-note {
  font-size: 11px;
  color: var(--text-faint);
  margin: 0 0 20px;
}

.transport {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 20px;
}

.tbtn {
  font-size: 18px;
  padding: 8px 14px;
}

.tbtn.main {
  font-size: 22px;
}

.tbtn.on {
  color: #0a84ff;
  background: rgba(10, 132, 255, 0.14);
}

.extras {
  display: flex;
  align-items: center;
  gap: 16px;
  flex-wrap: wrap;
  margin-bottom: 16px;
}

.vol,
.fmt {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 12px;
  color: var(--text-dim);
}

.vol input {
  width: 120px;
}

.vol input.ignored {
  opacity: 0.4;
}

.art-note {
  margin-top: 16px;
}

@media (max-width: 720px) {
  .np {
    flex-direction: column;
  }
}
</style>
