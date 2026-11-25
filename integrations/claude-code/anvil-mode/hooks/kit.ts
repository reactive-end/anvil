import type { test } from 'claude-code/testing'

type Body = Extract<Parameters<typeof test>[1], (...args: never[]) => unknown>
export type Engine = Parameters<Body>[0]
export type On = Parameters<Body>[1]

export const BAND = {
  plugin: 'anvil-mode',
  component: 'AbovePrompt',
  requestId: 'band',
  props: {
    hasSurvey: false,
    isCollapsed: false,
    isWorking: false,
    maxRows: 10,
    bodyColumns: 100,
    scroll: { offset: 0, bodyRows: 10 },
    view: {},
  },
} as const

export const PROMPT = { text: 'hi', wait: false, origin: { kind: 'composer' } } as const

export const START = { cwd: '/repo', surface: 'desktop', isInteractive: true } as const

export const REG_LANGUAGES = `
HKEY_CURRENT_USER\\Control Panel\\International\\User Profile
    Languages    REG_MULTI_SZ    es-US\\0es-ES\\0en-US
`

const CATALOG_DIR = '/cfg/cache/model-catalog'

const levels = (ids: string[], badges: Record<string, { message: string; variant: string }> = {}) => ({
  type: 'effort',
  effort_options: ids.map(id => ({ id, name: id, ...(badges[id] ? { badge: badges[id] } : {}) })),
})

const ALL = ['low', 'medium', 'high', 'xhigh', 'max']

/** A fabricated picker cache: nothing of the real one. */
export function catalogFile(fetchedAt: number, opus = 'Opus 5.5') {
  return JSON.stringify({
    version: 1,
    fetchedAt,
    catalog: {
      config: {
        models: [
          {
            id: 'claude-opus-5-5',
            name: opus,
            section: 'main',
            thinking: levels(ALL, {
              medium: { message: 'Recommended', variant: 'neutral' },
              max: { message: '3.5× or more usage', variant: 'warning' },
            }),
          },
          { id: 'claude-sonnet-5-5', name: 'Sonnet 5.5', section: 'main', thinking: levels(ALL) },
          {
            id: 'claude-opus-4-6',
            name: 'Opus 4.6',
            section: 'overflow',
            thinking: levels(['low', 'medium', 'high', 'max'], {
              high: { message: 'Recommended', variant: 'neutral' },
            }),
            runtime: { default_effort: 'high' },
          },
          {
            id: 'claude-haiku-4-5-20251001',
            name: 'Haiku 4.5',
            section: 'overflow',
            thinking: { type: 'none' },
          },
        ],
        provider_alias_targets: {
          opus: { default: 'claude-opus-5-5' },
          sonnet: { default: 'claude-sonnet-5-5' },
        },
      },
    },
  })
}

export function status(
  enabled: boolean,
  initialized = true,
  models: { orchestrator?: string; executor?: string } = {},
) {
  return {
    initialized,
    enabled,
    orchestrator: { model: models.orchestrator ?? 'opus', effort: 'xhigh' },
    executor: { model: models.executor ?? 'sonnet', effort: 'medium' },
  }
}

export function ran(stdout: string, exitCode = 0, stderr = '') {
  return {
    value: { exitCode, stdout, stderr, isStdoutTruncated: false, isStderrTruncated: false },
  }
}

/** The picks run `anvil` after the gesture returns: let those calls land. */
export async function settled(isDone: () => Promise<boolean>) {
  for (let turn = 0; turn < 50 && !(await isDone()); turn += 1) {
    await Promise.resolve()
  }
}

export type World = {
  enabled: boolean
  calls: string[]
  toasts: string[]
  fail: boolean
  executorEffort: string
  executorModel: string
  orchestratorModel: string
}

export type WorldOptions = {
  enabled?: boolean
  isProbeHung?: boolean
  isCatalogHung?: boolean
  registry?: string
  vars?: Record<string, string>
  /** Catalog files: name, text and mtime. */
  files?: { name: string; text: string; mtimeMs: number }[]
  executorEffort?: string
  executorModel?: string
  orchestratorModel?: string
}

/** A repo, `reg` printing `registry`, env vars `vars`, catalog `files` in the config dir. */
export function world(on: On, options: WorldOptions = {}) {
  const state: World = {
    enabled: options.enabled ?? false,
    calls: [],
    toasts: [],
    fail: false,
    executorEffort: options.executorEffort ?? 'medium',
    executorModel: options.executorModel ?? 'sonnet',
    orchestratorModel: options.orchestratorModel ?? 'opus',
  }
  const vars: Record<string, string | undefined> = {
    ...(options.files ? { CLAUDE_CONFIG_DIR: '/cfg' } : {}),
    ...options.vars,
  }
  // The engine may hand paths back resolved: compare their tail only.
  const tail = (path: string) => path.replaceAll('\\', '/')
  on('command.register', () => ({ value: { command: 'anvil-mode' } }))
  on('session.start', (_, e) => ({ cwd: e.cwd }))
  on('session.root', () => ({ value: '/repo' }))
  on('env.get', (_, e) => ({ value: vars[e.name] }))
  on('fs.list', (_, e) => {
    if (!tail(e.path).endsWith(CATALOG_DIR) || !options.files) {
      throw new Error('no such folder')
    }

    return {
      value: options.files.map(file => ({
        name: file.name,
        kind: 'file' as const,
        size: file.text.length,
        mtimeMs: file.mtimeMs,
        isLink: false,
      })),
    }
  })
  on('fs.read', (_, e) => {
    if (options.isCatalogHung) {
      return new Promise<never>(() => undefined)
    }

    const file = options.files?.find(known => tail(e.path).endsWith(`${CATALOG_DIR}/${known.name}`))

    if (file === undefined) {
      throw new Error('no such file')
    }

    return { value: file.text }
  })
  on('process.run', (_, e) => {
    // The OS probes (`reg`, `defaults`) answer `registry`, or fail when unset.
    if (e.argv[0] === 'reg' || e.argv[0] === 'defaults') {
      if (options.isProbeHung) {
        return new Promise<never>(() => undefined)
      }

      return ran(options.registry ?? '', options.registry === undefined ? 1 : 0)
    }

    const args = e.argv.slice(4)
    state.calls.push(args.join(' '))

    if (state.fail && args[0] === 'setup') {
      return ran('', 1, 'boom')
    }

    if (args[0] === 'on' || args[0] === 'off') {
      state.enabled = args[0] === 'on'
    }

    // Anvil keeps what `setup` was given: the status reports it back.
    if (args[0] === 'setup') {
      for (let at = 1; at < args.length; at += 2) {
        const value = args[at + 1] ?? ''
        const key = args[at]

        if (key === '--executor-effort') state.executorEffort = value
        if (key === '--executor-model') state.executorModel = value
        if (key === '--model') state.orchestratorModel = value
      }
    }

    const now = status(state.enabled, true, {
      orchestrator: state.orchestratorModel,
      executor: state.executorModel,
    })
    now.executor.effort = state.executorEffort

    return ran(JSON.stringify(now))
  })
  on('prompt.submit', (_, e) => ({ text: e.text }))
  on('ui.toast', (_, e) => {
    state.toasts.push(e.text)

    return { value: undefined }
  })

  return state
}

export async function mount($: Engine, on: On, options?: WorldOptions) {
  const state = world(on, options)
  await $.session.start(START)
  await $.prompt.submit(PROMPT)

  const ui = await $.ui.mount({ ...BAND, surface: 'desktop' })

  // Detection and the catalog load run detached from the start and the first draw.
  for (let turn = 0; turn < 12; turn += 1) {
    await settled(async () => false)
  }

  return { state, ui }
}
