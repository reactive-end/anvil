export type Effort = 'low' | 'medium' | 'high' | 'xhigh' | 'max'

export type Role = { model: string; effort: Effort }

export type Mode = {
  initialized: boolean
  enabled: boolean
  orchestrator: Role
  executor: Role
}

/** Languages the band speaks; any other machine language falls back to `en`. */
export type Lang = 'es' | 'en'

export type EffortOption = { id: Effort }

/** `efforts` is empty for a model that takes no effort. */
export type ModelInfo = {
  id: string
  name: string
  section: 'main' | 'overflow'
  efforts: EffortOption[]
  defaultEffort?: Effort
}

/** What the desktop app's picker offers; `aliases` maps `opus` to its full id. */
export type Catalog = { models: ModelInfo[]; aliases: Record<string, string> }

declare module 'claude-code' {
  interface PluginState {
    'anvil-mode': {
      mode: Shaped<Mode | null>
      lang: Lang | null
      catalog: Shaped<Catalog | null>
    }
  }
}
