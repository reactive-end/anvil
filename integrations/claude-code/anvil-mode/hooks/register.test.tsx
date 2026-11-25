import { expect, test } from 'claude-code/testing'

import { BAND, PROMPT, REG_LANGUAGES, START, mount, ran, settled, status } from './kit'

/** A Select's options as `value:label`, in order. */
async function options(ui: Awaited<ReturnType<typeof mount>>['ui'], key: string) {
  const props = (await ui.find({ key }))?.props as { options?: { value: string; label: string }[] }

  return props?.options?.map(option => `${option.value}:${option.label}`)
}

const prop = async (ui: Awaited<ReturnType<typeof mount>>['ui'], key: string, name: string) =>
  ((await ui.find({ key }))?.props as Record<string, unknown> | undefined)?.[name]

test('mode off, in Spanish: one button and nothing else', async ($, on) => {
  const { state, ui } = await mount($, on, { registry: REG_LANGUAGES })
  expect((await ui.find({ key: 'mode' }))?.text).toBe('Activar orquestador')
  expect(await ui.find({ type: 'Select' })).toBeUndefined()
  expect(await ui.find({ type: 'Text' })).toBeUndefined()
  expect(await ui.find({ type: 'Svg' })).toBeUndefined()

  await ui.press({ key: 'mode' })
  await settled(async () => state.calls.includes('on'))
  await settled(async () => (await ui.find({ key: 'executor-model' })) !== undefined)
  expect(state.calls).toContain('on')
  expect((await ui.find({ key: 'mode' }))?.text).toBe('Desactivar')
  await ui.unmount()
})

test('mode on, in Spanish: the executor selects and the off button, no other text', async ($, on) => {
  const { ui } = await mount($, on, { registry: REG_LANGUAGES, enabled: true })
  expect(await prop(ui, 'executor-model', 'label')).toBe('Ejecutor')
  expect(await prop(ui, 'executor-effort', 'label')).toBeUndefined()
  expect(await prop(ui, 'executor-effort', 'value')).toBe('medium')
  expect(await ui.find({ key: 'orchestrator-model' })).toBeUndefined()
  expect(await ui.find({ key: 'orchestrator-effort' })).toBeUndefined()
  expect(await options(ui, 'executor-effort')).toEqual([
    'low:Bajo', 'medium:Medio', 'high:Alto', 'xhigh:Extra', 'max:Máx',
  ])
  expect(await options(ui, 'executor-model')).toEqual([
    'fable:Fable', 'opus:Opus', 'sonnet:Sonnet', 'haiku:Haiku',
  ])
  expect((await ui.find({ key: 'mode' }))?.text).toBe('Desactivar')
  expect(await ui.find({ type: 'Text' })).toBeUndefined()
  expect(await ui.find({ type: 'Svg' })).toBeUndefined()
  await ui.unmount()
})

test('English for an unknown locale', async ($, on) => {
  const { ui } = await mount($, on, { vars: { LANG: 'fr_FR.UTF-8' }, enabled: true })
  expect(await prop(ui, 'executor-model', 'label')).toBe('Executor')
  expect((await ui.find({ key: 'mode' }))?.text).toBe('Turn off')
  expect(await options(ui, 'executor-effort')).toContain('low:Low')
  await ui.unmount()
})

test('session.start and the first draw never wait for a language probe', async ($, on) => {
  const { state } = await mount($, on, { enabled: true, isProbeHung: true })
  const ui = await $.ui.mount({ ...BAND, surface: 'desktop', requestId: 'again' })
  expect((await ui.find({ key: 'mode' }))?.text).toBe('Turn off')
  expect(state.calls).toContain('status --json')
  await ui.unmount()
})

test('POSIX environment picks Spanish from LANG', async ($, on) => {
  const { ui } = await mount($, on, { vars: { LANG: 'es_CL.UTF-8' } })
  // The kit does not redraw on its own after the `lang` write: a press does.
  await ui.press({ key: 'mode' })
  expect((await ui.find({ key: 'mode' }))?.text).toBe('Desactivar')
  await ui.unmount()
})

test('picking an effort shows it at once and runs setup', async ($, on) => {
  const { state, ui } = await mount($, on, { registry: REG_LANGUAGES, enabled: true })
  await ui.select({ key: 'executor-effort', value: 'high' })
  await settled(async () => state.calls.includes('setup --executor-effort high'))
  expect(state.calls).toContain('setup --executor-effort high')
  expect(await prop(ui, 'executor-effort', 'value')).toBe('high')
  await ui.unmount()
})

test('picking a model runs setup', async ($, on) => {
  const { state, ui } = await mount($, on, { registry: REG_LANGUAGES, enabled: true })
  await ui.select({ key: 'executor-model', value: 'fable' })
  await settled(async () => state.calls.includes('setup --executor-model fable'))
  expect(state.calls).toContain('setup --executor-model fable')
  expect(await prop(ui, 'executor-model', 'value')).toBe('fable')
  await ui.unmount()
})

test('a failed pick shows the localized toast and the true state', async ($, on) => {
  const { state, ui } = await mount($, on, { registry: REG_LANGUAGES, enabled: true })
  state.fail = true
  await ui.select({ key: 'executor-model', value: 'haiku' })
  await settled(async () => state.toasts.length > 0)
  expect(state.toasts).toEqual(['Anvil: boom'])
  await settled(async () => (await prop(ui, 'executor-model', 'value')) === 'sonnet')
  expect(await prop(ui, 'executor-model', 'value')).toBe('sonnet')
  await ui.unmount()
})

test('the main thread keeps the session picker; only the executor gets the band model', async ($, on) => {
  const sent: string[] = []
  const spawned: string[] = []
  on('command.register', () => ({ value: { command: 'anvil-mode' } }))
  on('session.start', (_, e) => ({ cwd: e.cwd }))
  on('session.root', () => ({ value: '/repo' }))
  on('process.run', () => ran(JSON.stringify(status(true))))
  on('agent.spawn', (_, e) => {
    spawned.push(`${e.subagentType} ${e.model}`)

    return { model: e.model ?? 'unset' }
  })
  on('turn.step', async function* (_, e) {
    sent.push(`${e.agentId ?? 'main'} ${e.model} ${e.effort}`)
    yield* []

    return {
      turnId: e.turnId,
      index: e.index,
      answer: '',
      toolUses: [],
      stopReason: null,
      usage: null,
    }
  })
  on('prompt.submit', (_, e) => ({ text: e.text }))
  await $.session.start(START)
  await $.prompt.submit(PROMPT)

  const step = { turnId: 't', index: 0, model: 'haiku', effort: 'low', messageCount: 1 } as const

  for await (const _ of $.turn.step(step)) {
  }

  for await (const _ of $.turn.step({ ...step, agentId: 'a1' })) {
  }

  expect(sent).toEqual(['main haiku low', 'a1 haiku low'])

  for (const subagentType of ['anvil-executor', 'anvil:anvil-executor', 'explorer']) {
    await $.agent.spawn({
      tool_use_id: 'u1',
      prompt: 'x',
      description: 'x',
      subagentType,
      provider: { plugin: 'test', tier: 'user' },
      model: 'haiku',
      parentModel: 'haiku',
      background: false,
      fork: false,
    })
  }

  expect(spawned).toEqual([
    'anvil-executor sonnet',
    'anvil:anvil-executor sonnet',
    'explorer haiku',
  ])
})

test('outside an Anvil repository nothing is drawn', async ($, on) => {
  on('command.register', () => ({ value: { command: 'anvil-mode' } }))
  on('session.start', (_, e) => ({ cwd: e.cwd }))
  on('session.root', () => ({ value: '/repo' }))
  on('process.run', () => ran(JSON.stringify(status(false, false))))
  on('ui.render', ($, e) => {
    const { Text } = $.ui.resolve(e)

    return <Text>engine</Text>
  })
  await $.session.start(START)

  const ui = await $.ui.mount({ ...BAND, surface: 'desktop' })
  expect((await ui.find({ type: 'Text', text: /engine/ }))?.text).toBe('engine')
  await ui.unmount()
})
