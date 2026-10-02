import { Pause, Play } from "lucide-vue-next";
import { computed, markRaw, reactive } from "vue";
import { usePlayerStore } from "../stores/player";
import type { Track } from "../types";

const PLAY = markRaw(Play);
const PAUSE = markRaw(Pause);

/**
 * A Play button that knows what is playing. When the current track is the
 * button's (its album, playlist, genre, book or the track itself) and it is
 * playing, the button says Pause and pauses; when it is the button's and
 * paused, Play carries on from there instead of starting over. Otherwise Play
 * does what it always did (`start`).
 */
export function usePlayToggle(isThis: (current: Track) => boolean, start: () => unknown) {
  const player = usePlayerStore();
  const current = computed(() => player.currentTrack !== null && isThis(player.currentTrack));
  const playing = computed(() => current.value && player.isPlaying);

  function press(): void {
    if (playing.value) void player.pause();
    else if (current.value && player.status === "paused") void player.resume();
    else void start();
  }

  return reactive({
    /** The button's own track is playing: show Pause. */
    playing,
    label: computed(() => (playing.value ? "Pause" : "Play")),
    icon: computed(() => (playing.value ? PAUSE : PLAY)),
    press,
  });
}
