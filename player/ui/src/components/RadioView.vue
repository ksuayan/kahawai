<script setup lang="ts">
import { ArrowDown, ArrowUp, Star, Trash2 } from "lucide-vue-next";
import { computed, onMounted, ref, watch } from "vue";
import { RADIO_SORTS, sortStations } from "../lib/radioSort";
import { useRadioStore } from "../stores/radio";
import { useViewPrefsStore } from "../stores/viewPrefs";
import StateMessage from "../ui/StateMessage.vue";
import UiButton from "../ui/UiButton.vue";
import UiHint from "../ui/UiHint.vue";
import UiInput from "../ui/UiInput.vue";
import UiSelect, { type UiSelectOption } from "../ui/UiSelect.vue";
import ViewShell from "../ui/ViewShell.vue";
import ListToolbar from "./ListToolbar.vue";
import RadioStationRow from "./RadioStationRow.vue";

const radio = useRadioStore();
const view = useViewPrefsStore();
/** Shown in the chosen order; "As listed" keeps your favorites' own order (and the directory's). */
const favorites = computed(() => sortStations(radio.favorites, view.prefs.radioSort));
const results = computed(() => sortStations(radio.results, view.prefs.radioSort));
const ownOrder = computed(() => view.prefs.radioSort === "listed");
const listClass = computed(() =>
  view.prefs.radioLayout === "grid" ? "m-0 grid list-none grid-cols-[repeat(auto-fill,minmax(160px,1fr))] gap-3 p-0" : "m-0 list-none p-0",
);
const tab = ref<"favorites" | "browse">("favorites");
const address = ref("");
const adding = ref(false);
const addError = ref<string | null>(null);

onMounted(() => void radio.loadFavorites());

// Opening the directory loads the pickers; the first visit shows the popular stations.
watch(tab, (t) => {
  if (t !== "browse") return;
  void radio.loadFacets();
  if (!radio.searched) void radio.search();
});

let timer: number | undefined;
watch(
  () => [radio.query.q, radio.query.tag, radio.query.country, radio.query.language, radio.query.order],
  () => {
    if (tab.value !== "browse") return;
    window.clearTimeout(timer);
    timer = window.setTimeout(() => void radio.search(), 300);
  },
);

const opt = (label: string, list: { name: string; stations: number }[]): UiSelectOption[] => [
  { value: null, label },
  ...list.slice(0, 200).map((f) => ({ value: f.name, label: `${f.name} (${f.stations.toLocaleString()})` })),
];
const tagOptions = computed(() => opt("Any genre", radio.facets.tags));
const countryOptions = computed(() => opt("Any country", radio.facets.countries));
const languageOptions = computed(() => opt("Any language", radio.facets.languages));
const orderOptions: UiSelectOption[] = [
  { value: "clickcount", label: "Most listened" },
  { value: "votes", label: "Most voted" },
  { value: "bitrate", label: "Highest bitrate" },
  { value: "name", label: "Name" },
];

async function add(): Promise<void> {
  if (!address.value.trim()) return;
  adding.value = true;
  addError.value = null;
  try {
    await radio.addByAddress(address.value);
    address.value = "";
  } catch (e) {
    addError.value = e instanceof Error ? e.message : String(e);
  } finally {
    adding.value = false;
  }
}

const sub = (parts: (string | null | undefined)[]): string => parts.filter(Boolean).join(" · ");
const tags = (t: string | null): string | null => (t ? t.split(",").slice(0, 3).join(", ") : null);
const playingId = computed(() => radio.stationFavorite?.id ?? null);
</script>

<template>
  <ViewShell title="Radio" subtitle="Internet radio stations" width="wide">
    <template #actions>
      <div class="flex gap-1" role="tablist" aria-label="Radio">
        <UiButton role="tab" :pressed="tab === 'favorites'" :variant="tab === 'favorites' ? 'primary' : 'default'" data-testid="tab-favorites" @click="tab = 'favorites'">
          Favorites
        </UiButton>
        <UiButton role="tab" :variant="tab === 'browse' ? 'primary' : 'default'" data-testid="tab-browse" @click="tab = 'browse'">
          Find stations
        </UiButton>
      </div>
      <ListToolbar
        v-model:layout="view.prefs.radioLayout"
        v-model:sort="view.prefs.radioSort"
        :sort-options="RADIO_SORTS"
        sort-label="Sort by"
      />
    </template>

    <!-- Add a station by its stream address: first, under the heading. -->
    <section class="mb-4" aria-label="Add by address" data-testid="add-by-address">
      <div class="flex gap-2">
        <UiInput v-model="address" class="flex-1" type="text" spellcheck="false" placeholder="Add a station by its stream address: https://example.com/live.mp3" aria-label="Stream address" data-testid="station-address" @keydown.enter="add" />
        <UiButton variant="primary" :disabled="adding || !address.trim()" data-testid="add-station" @click="add">Add</UiButton>
      </div>
      <UiHint>The address of the stream itself (it ends in <code>.mp3</code>, <code>.aac</code>, a port number, or <code>/stream</code>), not a playlist file or a web page.</UiHint>
      <StateMessage v-if="addError" kind="error" class="mt-2">{{ addError }}</StateMessage>
    </section>

    <StateMessage v-if="radio.error" kind="error">{{ radio.error }}</StateMessage>

    <!-- Favorites -->
    <section v-if="tab === 'favorites'" data-testid="favorites">
      <ul v-if="radio.favorites.length" :class="listClass" data-testid="favorites-list">
        <RadioStationRow
          v-for="(f, i) in favorites"
          :key="f.id"
          :name="f.name"
          :subtitle="sub([tags(f.tags), f.country, f.manual ? 'Added by address' : null])"
          :favicon="f.favicon"
          :codec="f.codec"
          :bitrate="f.bitrate"
          :playing="playingId === f.id"
          :layout="view.prefs.radioLayout"
          @play="radio.play(f)"
        >
          <UiButton variant="icon" :title="ownOrder ? 'Move up' : 'Choose Sort by: As listed to change your order'" aria-label="Move up" :disabled="!ownOrder || i === 0" data-testid="move-up" @click="radio.move(f.id, -1)"><ArrowUp /></UiButton>
          <UiButton variant="icon" :title="ownOrder ? 'Move down' : 'Choose Sort by: As listed to change your order'" aria-label="Move down" :disabled="!ownOrder || i === radio.favorites.length - 1" data-testid="move-down" @click="radio.move(f.id, 1)"><ArrowDown /></UiButton>
          <UiButton variant="icon-danger" title="Remove from favorites" aria-label="Remove from favorites" data-testid="remove-favorite" @click="radio.removeFavorite(f.id)"><Trash2 /></UiButton>
        </RadioStationRow>
      </ul>
      <StateMessage v-else-if="radio.favoritesLoaded" kind="empty">
        No favorite stations yet. Find some under Find stations, or add one by its address above.
      </StateMessage>
    </section>

    <!-- Directory -->
    <section v-else data-testid="browse">
      <StateMessage v-if="radio.directoryOff" kind="empty" data-testid="directory-off">
        The online station directory is off. To search it, turn on “Online sources” in the Kahawai Server's
        Settings. Stations you have saved or added by address still play.
      </StateMessage>
      <template v-else>
        <div class="mb-3 flex flex-wrap gap-2">
          <UiInput v-model="radio.query.q" class="w-[220px]" type="search" placeholder="Station name…" aria-label="Search stations" data-testid="station-search" />
          <UiSelect aria-label="Genre" trigger-class="w-[170px]" :model-value="radio.query.tag || null" :options="tagOptions" @update:model-value="(v) => (radio.query.tag = v ?? '')" />
          <UiSelect aria-label="Country" trigger-class="w-[170px]" :model-value="radio.query.country || null" :options="countryOptions" @update:model-value="(v) => (radio.query.country = v ?? '')" />
          <UiSelect aria-label="Language" trigger-class="w-[170px]" :model-value="radio.query.language || null" :options="languageOptions" @update:model-value="(v) => (radio.query.language = v ?? '')" />
          <UiSelect aria-label="Which stations" title="Which stations the directory sends" trigger-class="w-[150px]" :model-value="radio.query.order" :options="orderOptions" @update:model-value="(v) => (radio.query.order = (v ?? 'clickcount') as typeof radio.query.order)" />
        </div>
        <StateMessage v-if="radio.searching && !radio.results.length" kind="loading">Looking for stations…</StateMessage>
        <StateMessage v-else-if="radio.searched && !radio.results.length && !radio.error" kind="empty">No station matches.</StateMessage>
        <ul v-else :class="listClass" data-testid="results">
          <RadioStationRow
            v-for="s in results"
            :key="s.station_uuid || s.url"
            :name="s.name"
            :subtitle="sub([tags(s.tags), s.country, s.clicks ? `${s.clicks.toLocaleString()} listens` : null])"
            :favicon="s.favicon"
            :codec="s.codec"
            :bitrate="s.bitrate"
            :disabled-reason="s.hls ? 'HLS streams are not supported yet' : null"
            :layout="view.prefs.radioLayout"
            @play="radio.playStation(s)"
          >
            <UiButton
              variant="icon"
              :pressed="!!radio.favoriteOf(s)"
              :title="radio.favoriteOf(s) ? 'In your favorites' : 'Add to favorites'"
              :aria-label="radio.favoriteOf(s) ? 'In your favorites' : 'Add to favorites'"
              :disabled="!!radio.favoriteOf(s)"
              data-testid="favorite-station"
              @click="radio.addFavorite(s)"
            >
              <Star :class="radio.favoriteOf(s) && 'fill-current'" />
            </UiButton>
          </RadioStationRow>
        </ul>
      </template>
    </section>

    <section v-if="radio.heard.length" class="mt-6" data-testid="heard">
      <h3 class="heading-3 mb-1">Heard on {{ radio.stationName }}</h3>
      <ul class="m-0 list-none p-0 text-[13px] text-dim">
        <li v-for="(t, i) in radio.heard" :key="`${i}-${t}`" class="truncate py-0.5">{{ t }}</li>
      </ul>
    </section>
  </ViewShell>
</template>
