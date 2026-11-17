<template>
  <UiSelect v-if="s.drives.length > 1" v-model="s.driveKey" :label="t('drive.label')" :options="options" :disabled="!!s.job" />
  <div class="small muted">{{ status }}<span v-if="mid" class="mono mid" :data-tooltip="t('drive.midTip')"> · {{ mid }}</span></div>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import { t } from '../i18n'
import { useStudio } from '../stores/studio'
import UiSelect from './UiSelect.vue'

const s = useStudio()
// The model name only matters when it has to tell two drives apart.
const options = computed(() => s.drives.map((d) => ({ value: s.keyOf(d), label: d.simulated ? d.product : `${d.vendor} ${d.product}` })))
// The disc names itself; the drive only says what it can see of it.
const mid = computed(() => s.drive?.mediaId ?? '')
const status = computed(() => {
  const d = s.drive
  if (!d) return t('drive.none')
  if (!d.mediaPresent) return t('drive.noDisc')
  if (!d.supported) return t('drive.unsupported', { type: d.mediaType })
  return t(d.isBlank ? 'drive.blank' : 'drive.written')
})
</script>

<style scoped>
.mid { margin-left: 0.3em; }
</style>
