import type { Catalog, Effort, EffortOption, Mode, ModelInfo } from '../types'

export const EFFORTS: readonly Effort[] = ['low', 'medium', 'high', 'xhigh', 'max']
export const ALIASES = ['fable', 'opus', 'sonnet', 'haiku']

const SAFE_ID = /^claude-[A-Za-z0-9.\-[\]]+$/

const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === 'object' && value !== null && !Array.isArray(value)

const asEffort = (value: unknown): Effort | undefined =>
  EFFORTS.find(effort => effort === value)

function parseEfforts(thinking: unknown): EffortOption[] {
  if (!isRecord(thinking) || thinking.type !== 'effort' || !Array.isArray(thinking.effort_options)) {
    return []
  }

  const seen = new Set<Effort>()
  const options: EffortOption[] = []

  for (const raw of thinking.effort_options as unknown[]) {
    const id = isRecord(raw) ? asEffort(raw.id) : undefined

    if (id === undefined || seen.has(id)) {
      continue
    }

    seen.add(id)
    options.push({ id })
  }

  return options
}

function parseModel(raw: unknown): ModelInfo | null {
  if (!isRecord(raw) || typeof raw.id !== 'string' || !SAFE_ID.test(raw.id)) {
    return null
  }

  if (typeof raw.name !== 'string' || raw.name.trim() === '') {
    return null
  }

  if (raw.section !== 'main' && raw.section !== 'overflow') {
    return null
  }

  const efforts = parseEfforts(raw.thinking)
  const runtime = isRecord(raw.runtime) ? raw.runtime : {}
  const preferred = asEffort(runtime.default_effort)
  const info: ModelInfo = { id: raw.id, name: raw.name.trim(), section: raw.section, efforts }

  if (preferred !== undefined && efforts.some(option => option.id === preferred)) {
    info.defaultEffort = preferred
  }

  return info
}

/** The app's picker cache is undocumented: anything unexpected is dropped, never thrown. */
export function parseCatalog(text: string): { fetchedAt: number; catalog: Catalog } | null {
  let json: unknown

  try {
    json = JSON.parse(text)
  } catch {
    return null
  }

  const config = isRecord(json) && isRecord(json.catalog) ? json.catalog.config : undefined

  if (!isRecord(config) || !Array.isArray(config.models)) {
    return null
  }

  const models: ModelInfo[] = []

  for (const raw of config.models as unknown[]) {
    const model = parseModel(raw)

    if (model !== null && !models.some(known => known.id === model.id)) {
      models.push(model)
    }
  }

  if (models.length === 0) {
    return null
  }

  const aliases: Record<string, string> = {}
  const targets = isRecord(config.provider_alias_targets) ? config.provider_alias_targets : {}

  for (const [alias, target] of Object.entries(targets)) {
    const id = isRecord(target) ? target.default : undefined

    if (/^[a-z]+$/.test(alias) && typeof id === 'string' && SAFE_ID.test(id)) {
      aliases[alias] = id
    }
  }

  const fetchedAt = isRecord(json) && typeof json.fetchedAt === 'number' ? json.fetchedAt : 0

  return { fetchedAt, catalog: { models, aliases } }
}

/** The catalog of the file fetched last; files that do not parse are skipped. */
export function newestCatalog(texts: readonly string[]): Catalog | null {
  let best: { fetchedAt: number; catalog: Catalog } | null = null

  for (const text of texts) {
    const parsed = parseCatalog(text)

    if (parsed !== null && (best === null || parsed.fetchedAt > best.fetchedAt)) {
      best = parsed
    }
  }

  return best?.catalog ?? null
}

const capitalize = (value: string) => value.charAt(0).toUpperCase() + value.slice(1)

/** Today's choice when the app's catalog cannot be read: the four aliases, every level. */
export const FALLBACK: Catalog = {
  aliases: {},
  models: ALIASES.map(alias => ({
    id: alias,
    name: capitalize(alias),
    section: 'main',
    efforts: EFFORTS.map(id => ({ id })),
  })),
}

export const resolveModel = (catalog: Catalog, model: string) => catalog.aliases[model] ?? model

export const findModel = (catalog: Catalog, model: string) => {
  const id = resolveModel(catalog, model)

  return catalog.models.find(known => known.id === id)
}

/** The levels a model takes: none for a catalog model without effort, all for an unknown one. */
export function levelsOf(catalog: Catalog, model: string): readonly EffortOption[] {
  const info = findModel(catalog, model)

  return info === undefined ? EFFORTS.map(id => ({ id })) : info.efforts
}

/** The level to fall to when the new model lacks the current one. */
export function fitEffort(catalog: Catalog, model: string, current: Effort): Effort {
  const levels = levelsOf(catalog, model).map(option => option.id)

  if (levels.length === 0 || levels.includes(current)) {
    return current
  }

  const preferred = findModel(catalog, model)?.defaultEffort

  if (preferred !== undefined) {
    return preferred
  }

  const rank = EFFORTS.indexOf(current)
  const below = levels.filter(level => EFFORTS.indexOf(level) < rank)

  return below[below.length - 1] ?? levels[0] ?? current
}

/** The state after a pick of the executor and the one `setup` call that makes it, model and effort together. */
export function setupFor(
  catalog: Catalog,
  now: Mode,
  change: { model?: string; effort?: Effort },
): { next: Mode; args: string[] } {
  const model = change.model ?? now.executor.model
  const effort = change.effort ?? fitEffort(catalog, model, now.executor.effort)
  const args = ['setup']

  if (change.model !== undefined) {
    args.push('--executor-model', model)
  }

  if (change.effort !== undefined || effort !== now.executor.effort) {
    args.push('--executor-effort', effort)
  }

  return { next: { ...now, executor: { model, effort } }, args }
}
