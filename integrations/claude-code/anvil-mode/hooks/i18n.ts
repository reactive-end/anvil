import type { Effort, Lang } from '../types'

export type Strings = {
  turnOn: string
  turnOff: string
  executorRole: string
  sessionModel: string
  levels: Record<Effort, string>
  failed: (reason: string) => string
  unavailable: string
  commandDescription: string
  notInitialized: string
  modeOn: string
  modeOff: string
  noChange: string
}

export const STRINGS: Record<Lang, Strings> = {
  es: {
    turnOn: 'Activar orquestador',
    turnOff: 'Desactivar',
    executorRole: 'Ejecutor',
    sessionModel: 'El de la sesión',
    levels: { low: 'Bajo', medium: 'Medio', high: 'Alto', xhigh: 'Extra', max: 'Máx' },
    failed: reason => `Anvil: ${reason}`,
    unavailable: 'Anvil: el comando `anvil` no está disponible',
    commandDescription: 'Activa o desactiva el modo orquestador de Anvil',
    notInitialized: 'Anvil no está inicializado aquí. Ejecuta `anvil init` primero.',
    modeOn: 'Modo orquestador activado.',
    modeOff: 'Modo orquestador desactivado.',
    noChange: 'el cambio falló',
  },
  en: {
    turnOn: 'Turn on orchestrator',
    turnOff: 'Turn off',
    executorRole: 'Executor',
    sessionModel: 'Session model',
    levels: { low: 'Low', medium: 'Medium', high: 'High', xhigh: 'Extra', max: 'Max' },
    failed: reason => `Anvil: ${reason}`,
    unavailable: 'Anvil: the `anvil` command is not available',
    commandDescription: 'Turn Anvil orchestrator mode on or off',
    notInitialized: 'Anvil is not initialized here. Run `anvil init` first.',
    modeOn: 'Orchestrator mode on.',
    modeOff: 'Orchestrator mode off.',
    noChange: 'the change failed',
  },
}

const capitalize = (value: string) => value.charAt(0).toUpperCase() + value.slice(1)

/** The catalog's name when it has one; aliases are capitalized, other ids shown as they are. */
export function modelName(model: string, t: Strings, known?: string): string {
  if (model === 'inherit') {
    return t.sessionModel
  }

  return known ?? (model.startsWith('claude-') ? model : capitalize(model))
}

export const effortName = (effort: Effort, t: Strings) => t.levels[effort] ?? effort
