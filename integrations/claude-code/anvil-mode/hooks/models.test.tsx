import { expect, test } from 'claude-code/testing'

import { fitEffort, newestCatalog, parseCatalog, setupFor } from './catalog'
import { REG_LANGUAGES, catalogFile, mount, settled, status } from './kit'

const FILES = [{ name: 'acct-ccd.json', text: catalogFile(2000), mtimeMs: 1 }]

test('parser: good file, no-effort model, aliases', async () => {
  const parsed = parseCatalog(catalogFile(7))
  expect(parsed?.fetchedAt).toBe(7)
  expect(parsed?.catalog.models.map(model => model.id)).toEqual([
    'claude-opus-5-5',
    'claude-sonnet-5-5',
    'claude-opus-4-6',
    'claude-haiku-4-5-20251001',
  ])
  expect(parsed?.catalog.models[3]?.efforts).toEqual([])
  expect(parsed?.catalog.models[2]?.defaultEffort).toBe('high')
  expect(parsed?.catalog.models[0]?.efforts[4]).toEqual({ id: 'max' })
  expect(parsed?.catalog.aliases).toEqual({
    opus: 'claude-opus-5-5',
    sonnet: 'claude-sonnet-5-5',
  })
})

test('parser: malformed, duplicate and unsafe entries are dropped', async () => {
  const model = (extra: object) => ({
    id: 'claude-ok-1',
    name: 'Ok',
    section: 'main',
    thinking: { type: 'effort', effort_options: [{ id: 'low' }, { id: 'turbo' }, { id: 'low' }] },
    ...extra,
  })
  const file = (models: unknown[]) =>
    JSON.stringify({ fetchedAt: 1, catalog: { config: { models } } })
  const parsed = parseCatalog(
    file([
      model({}),
      model({ name: 'Duplicate' }),
      model({ id: 'claude-x;rm -rf' }),
      model({ id: 'gpt-5' }),
      model({ id: 'claude-noname', name: '' }),
      model({ id: 'claude-nosection', section: 'hidden' }),
      'text',
      null,
    ]),
  )
  expect(parsed?.catalog.models).toEqual([
    { id: 'claude-ok-1', name: 'Ok', section: 'main', efforts: [{ id: 'low' }] },
  ])
  expect(parseCatalog('not json')).toBeNull()
  expect(parseCatalog('{}')).toBeNull()
  expect(parseCatalog(file([]))).toBeNull()
})

test('the file fetched last wins, whatever its mtime', async () => {
  const older = catalogFile(1, 'Opus OLD')
  const newer = catalogFile(9, 'Opus NEW')
  expect(newestCatalog([newer, older, 'junk'])?.models[0]?.name).toBe('Opus NEW')
  expect(newestCatalog([older, newer])?.models[0]?.name).toBe('Opus NEW')
  expect(newestCatalog(['junk'])).toBeNull()
})

test('a model lacking the current effort falls to its default, else the next below', async () => {
  const catalog = newestCatalog([catalogFile(1)])

  if (catalog === null) {
    throw new Error('fixture does not parse')
  }

  expect(fitEffort(catalog, 'claude-opus-4-6', 'xhigh')).toBe('high')
  expect(fitEffort(catalog, 'claude-opus-4-6', 'medium')).toBe('medium')
  expect(fitEffort(catalog, 'claude-haiku-4-5-20251001', 'xhigh')).toBe('xhigh')
  const now = status(true) as Parameters<typeof setupFor>[1]
  expect(setupFor(catalog, now, { model: 'claude-opus-4-6' }).args).toEqual([
    'setup', '--executor-model', 'claude-opus-4-6',
  ])
})

type Ui = Awaited<ReturnType<typeof mount>>['ui']

async function options(ui: Ui, key: string) {
  const props = (await ui.find({ key }))?.props as { options?: { value: string; label: string }[] }

  return props?.options?.map(option => `${option.value}:${option.label}`)
}

const value = async (ui: Ui, key: string) =>
  ((await ui.find({ key }))?.props as { value?: string } | undefined)?.value

test('aliases resolve to their catalog id and name, main models first', async ($, on) => {
  const { ui } = await mount($, on, { enabled: true, files: FILES })
  expect(await value(ui, 'executor-model')).toBe('claude-sonnet-5-5')
  expect(await options(ui, 'executor-model')).toEqual([
    'claude-opus-5-5:Opus 5.5',
    'claude-sonnet-5-5:Sonnet 5.5',
    'claude-opus-4-6:Opus 4.6',
    'claude-haiku-4-5-20251001:Haiku 4.5',
  ])
  await ui.unmount()
})

test('a model the catalog lacks is listed', async ($, on) => {
  const { ui } = await mount($, on, {
    enabled: true,
    files: FILES,
    executorModel: 'claude-future-9',
  })
  expect(await value(ui, 'executor-model')).toBe('claude-future-9')
  expect((await options(ui, 'executor-model'))?.at(-1)).toBe('claude-future-9:claude-future-9')
  await ui.unmount()
})

test('inherit keeps its session-model label', async ($, on) => {
  const { ui } = await mount($, on, { enabled: true, files: FILES, executorModel: 'inherit' })
  expect((await options(ui, 'executor-model'))?.at(-1)).toBe('inherit:Session model')
  await ui.unmount()
})

test('a model without effort has no effort select', async ($, on) => {
  const { ui } = await mount($, on, {
    enabled: true,
    files: FILES,
    executorModel: 'claude-haiku-4-5-20251001',
  })
  expect(await ui.find({ key: 'executor-effort' })).toBeUndefined()
  expect(await ui.find({ key: 'executor-model' })).toBeDefined()
  await ui.unmount()
})

test('levels come from the selected model, in Spanish, keeping a stored one the model lacks', async ($, on) => {
  const { ui } = await mount($, on, {
    enabled: true,
    files: FILES,
    registry: REG_LANGUAGES,
    executorModel: 'claude-opus-4-6',
    executorEffort: 'xhigh',
  })
  expect(await options(ui, 'executor-effort')).toEqual([
    'low:Bajo', 'medium:Medio', 'high:Alto', 'max:Máx', 'xhigh:Extra',
  ])
  expect(await value(ui, 'executor-effort')).toBe('xhigh')
  await ui.unmount()
})

test('picking a model that lacks the effort sets its default in the same call', async ($, on) => {
  const { state, ui } = await mount($, on, {
    enabled: true,
    files: FILES,
    executorEffort: 'xhigh',
  })
  await ui.select({ key: 'executor-model', value: 'claude-opus-4-6' })
  await settled(async () => state.calls.some(call => call.startsWith('setup')))
  expect(state.calls.filter(call => call.startsWith('setup'))).toEqual([
    'setup --executor-model claude-opus-4-6 --executor-effort high',
  ])
  expect(await value(ui, 'executor-model')).toBe('claude-opus-4-6')
  await ui.unmount()
})

test('a hanging catalog read blocks neither the start nor the draw', async ($, on) => {
  const { ui } = await mount($, on, { enabled: true, files: FILES, isCatalogHung: true })
  expect(await value(ui, 'executor-model')).toBe('sonnet')
  expect((await options(ui, 'executor-model'))?.[0]).toBe('fable:Fable')
  await ui.unmount()
})

test('with no catalog folder the four aliases are offered', async ($, on) => {
  const { ui } = await mount($, on, { enabled: true })
  expect(await options(ui, 'executor-model')).toEqual([
    'fable:Fable', 'opus:Opus', 'sonnet:Sonnet', 'haiku:Haiku',
  ])
  await ui.unmount()
})
