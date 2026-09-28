<script setup lang="ts">
import { computed } from "vue";
import UiButton from "../ui/UiButton.vue";
import { useSetupStore } from "../stores/setup";
import WizardWelcome from "./WizardWelcome.vue";
import WizardMusicFolders from "./WizardMusicFolders.vue";
import WizardDatabase from "./WizardDatabase.vue";
import WizardReview from "./WizardReview.vue";
import WizardDone from "./WizardDone.vue";

const setup = useSetupStore();

/** Whether Continue is enabled on the current step. Step 4 (Done) has its
 *  own action buttons instead of Back/Continue. */
const canContinue = computed(() => {
  switch (setup.step) {
    case 1:
      return setup.canLeaveFolders;
    case 2:
      return setup.canLeaveDatabase;
    case 3:
      return setup.bindLooksValid;
    default:
      return true;
  }
});

async function onContinue(): Promise<void> {
  if (setup.step === 3) {
    if (await setup.save()) setup.goNext();
    return;
  }
  setup.goNext();
}
</script>

<template>
  <div class="flex h-full flex-col">
    <div class="min-h-0 flex-1 overflow-y-auto px-8 py-8">
      <WizardWelcome v-if="setup.step === 0" />
      <WizardMusicFolders v-else-if="setup.step === 1" />
      <WizardDatabase v-else-if="setup.step === 2" />
      <WizardReview v-else-if="setup.step === 3" />
      <WizardDone v-else-if="setup.step === 4" />
    </div>

    <div v-if="setup.step < 4" class="flex items-center justify-between border-t border-line px-8 py-4">
      <UiButton :disabled="setup.step === 0" @click="setup.goBack()">Back</UiButton>
      <UiButton variant="primary" :disabled="!canContinue" @click="onContinue()">
        {{ setup.step === 3 ? "Save" : "Continue" }}
      </UiButton>
    </div>
  </div>
</template>
