<script setup lang="ts">
import { ref } from 'vue'
import { t } from '../i18n'
import { auth, setToken } from '../lib/auth'
import { rumu } from '../lib/api'
import RumuMark from './brand/RumuMark.vue'

const token = ref('')

function submit(): void {
  if (!token.value.trim()) return
  setToken(token.value)
  token.value = ''
  rumu.reconnect()
}
</script>

<template>
  <main class="grid h-full place-items-center px-6">
    <div class="w-full max-w-xs">
      <RumuMark :height="34" class="mb-10" />
      <!-- No answer from the server (down, or the installed app offline): a
           token would not help; the client keeps retrying on its own. -->
      <p v-if="auth.error === 'unreachable'" role="status" class="text-secondary text-ink-3">
        {{ t('common.auth.unreachable') }}
      </p>
      <form v-else-if="auth.status === 'required'" class="flex flex-col gap-4" @submit.prevent="submit">
        <label for="access-token" class="text-secondary font-semibold">{{ t('common.auth.token') }}</label>
        <input
          id="access-token"
          v-model="token"
          type="password"
          autocomplete="current-password"
          autofocus
          required
          :aria-invalid="auth.error === 'invalid'"
          :aria-describedby="auth.error ? 'auth-error' : undefined"
          class="w-full"
        />
        <p v-if="auth.error" id="auth-error" role="alert" class="text-secondary text-danger">
          {{ t('common.auth.invalid') }}
        </p>
        <button class="btn btn-primary self-start" :disabled="!token.trim()">
          {{ t('common.auth.connect') }}
        </button>
      </form>
    </div>
  </main>
</template>
