<script setup lang="ts">
// The notifications section: whether this browser shows system
// notifications, and for which moves of a session. Kept in this browser
// (lib/notify), so changes apply at once, without Apply.
import { t } from '../../i18n'
import { enableNotify, NOTIFY_EVENTS, notifyPrefs, notifySupported, permission } from '../../lib/notify'
import Switch from '../ui/Switch.vue'
import SettingGroup from './SettingGroup.vue'
import SettingRow from './SettingRow.vue'

const blocked = !notifySupported
</script>

<template>
  <div class="@container flex min-h-0 flex-1 flex-col gap-5 overflow-y-auto px-4 pt-4 pb-6 sm:px-6">
    <SettingGroup>
      <SettingRow :label="t('settings.notifyEnabled')" :disabled="blocked">
        <template #label>
          <span
            v-if="blocked || permission.value === 'denied'"
            class="rounded-xs bg-surface-2 px-1.5 text-micro font-semibold text-ink-3"
          >
            {{ t(blocked ? 'settings.notifyUnsupported' : 'settings.notifyDenied') }}
          </span>
        </template>
        <Switch
          :model-value="notifyPrefs.enabled"
          :label="t('settings.notifyEnabled')"
          :disabled="blocked"
          @update:model-value="enableNotify"
        />
      </SettingRow>
    </SettingGroup>

    <SettingGroup :title="t('settings.notifyEvents')">
      <SettingRow
        v-for="e in NOTIFY_EVENTS"
        :key="e"
        :label="t(`settings.notify_${e}`)"
        :disabled="!notifyPrefs.enabled"
      >
        <Switch
          v-model="notifyPrefs.events[e]"
          :label="t(`settings.notify_${e}`)"
          :disabled="!notifyPrefs.enabled"
        />
      </SettingRow>
    </SettingGroup>
  </div>
</template>
