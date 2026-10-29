// Teach vue-i18n the shape of our English catalog, so a mistyped key is a
// compile error rather than a runtime fallback. `export {}` keeps this a
// module: without it `declare module` would replace vue-i18n instead of
// augmenting it.
type MessageSchema = typeof import('./en').default

declare module 'vue-i18n' {
  export interface DefineLocaleMessage extends MessageSchema {}
}

export {}
