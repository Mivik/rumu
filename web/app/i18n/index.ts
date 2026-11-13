// Message catalogs, split by domain rather than by component: a file here
// owns a cluster of screens, so changing one panel touches one JSON file.
// English is the schema every key is typed from (see types.d.ts); add a file
// to both locale folders and register it in both maps below.
import { createI18n } from 'vue-i18n'

import enCards from './en/cards.json'
import enLibrary from './en/library.json'
import enAssistants from './en/assistants.json'
import enChat from './en/chat.json'
import enCommon from './en/common.json'
import enErrors from './en/errors.json'
import enTools from './en/tools.json'
import enUsage from './en/usage.json'
import enSettings from './en/settings.json'
import enWorkspaces from './en/workspaces.json'
import enProjects from './en/projects.json'

import zhCards from './zh-CN/cards.json'
import zhLibrary from './zh-CN/library.json'
import zhAssistants from './zh-CN/assistants.json'
import zhChat from './zh-CN/chat.json'
import zhCommon from './zh-CN/common.json'
import zhErrors from './zh-CN/errors.json'
import zhTools from './zh-CN/tools.json'
import zhUsage from './zh-CN/usage.json'
import zhSettings from './zh-CN/settings.json'
import zhWorkspaces from './zh-CN/workspaces.json'
import zhProjects from './zh-CN/projects.json'

const en = {
  common: enCommon,
  chat: enChat,
  assistants: enAssistants,
  tools: enTools,
  cards: enCards,
  library: enLibrary,
  usage: enUsage,
  workspaces: enWorkspaces,
  projects: enProjects,
  settings: enSettings,
  errors: enErrors,
}

const zhCN = {
  common: zhCommon,
  chat: zhChat,
  assistants: zhAssistants,
  tools: zhTools,
  cards: zhCards,
  library: zhLibrary,
  usage: zhUsage,
  workspaces: zhWorkspaces,
  projects: zhProjects,
  settings: zhSettings,
  errors: zhErrors,
}

/** The shape every locale must match. */
export type MessageSchema = typeof en

export const SUPPORTED_LOCALES = ['en', 'zh-CN'] as const
export type Locale = (typeof SUPPORTED_LOCALES)[number]

export const LOCALE_LABELS: Record<Locale, string> = {
  en: 'English',
  'zh-CN': '中文',
}

const STORAGE_KEY = 'rumu.locale'

function isLocale(value: string | null): value is Locale {
  return value !== null && (SUPPORTED_LOCALES as readonly string[]).includes(value)
}

/** Saved choice first, then the browser's preference list. */
function detectLocale(): Locale {
  if (typeof window === 'undefined') return 'en'
  const saved = window.localStorage.getItem(STORAGE_KEY)
  if (isLocale(saved)) return saved
  for (const lang of navigator.languages ?? []) {
    if (lang.startsWith('zh')) return 'zh-CN'
    if (lang.startsWith('en')) return 'en'
  }
  return 'en'
}

export const i18n = createI18n({
  legacy: false,
  locale: detectLocale(),
  fallbackLocale: 'en',
  messages: { en, 'zh-CN': zhCN },
})

/** Every dot path of the English catalog, e.g. "chat.composer.send". */
type LeafPaths<T, P extends string = ''> = {
  [K in keyof T & string]: T[K] extends string ? `${P}${K}` : LeafPaths<T[K], `${P}${K}.`>
}[keyof T & string]

export type MessageKey = LeafPaths<typeof en>

/** The one translation signature the app uses: a checked key plus named args.
 * vue-i18n's own `t` accepts any string, so this is what catches typos. */
export interface TFunction {
  (key: MessageKey, named?: Record<string, unknown>): string
}

/** The live locale, for callers that need to branch on it (e.g. casing). */
export const locale = i18n.global.locale

/** Translate from anywhere: components, the store, lib helpers. The function
 * reads the composer's locale on every call, so it stays reactive. */
export const t: TFunction = (key, named) => i18n.global.t(key, named as never)

export function setLocale(locale: Locale): void {
  i18n.global.locale.value = locale
  if (typeof window === 'undefined') return
  window.localStorage.setItem(STORAGE_KEY, locale)
  document.documentElement.lang = locale
}
