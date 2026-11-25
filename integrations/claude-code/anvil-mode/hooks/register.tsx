import { atom, read, update } from 'claude-code'
import type { EngineInterface, Register } from 'claude-code'

import type { Effort, Mode } from '../types'
import { FALLBACK, setupFor } from './catalog'
import { STRINGS } from './i18n'
import { detectLang } from './language'
import { loadCatalog } from './models'
import { band } from './view'
import type { Actions } from './view'

const mode = atom({ plugin: 'anvil-mode', key: 'mode' } as const, null, {
  shape: 'mode-1',
})
const lang = atom({ plugin: 'anvil-mode', key: 'lang' } as const, null)
const catalog = atom({ plugin: 'anvil-mode', key: 'catalog' } as const, null, {
  shape: 'catalog-1',
})

/** The language is detected detached; English until it lands. */
const strings = async ($: EngineInterface) => STRINGS[(await read($, lang)) ?? 'en']

let loading: Promise<void> | null = null

/** Starts a catalog load unless one runs; nobody waits on it, a failure keeps the last list. */
function reloadCatalog($: EngineInterface) {
  loading ??= loadCatalog({
    env: {
      CLAUDE_CONFIG_DIR: () => $.env.get('CLAUDE_CONFIG_DIR'),
      USERPROFILE: () => $.env.get('USERPROFILE'),
      HOME: () => $.env.get('HOME'),
    },
    list: dir => $.fs.list(dir),
    read: path => $.fs.read(path),
  })
    .then(async found => {
      if (found !== null) {
        await update($, catalog, () => found)
      }
    })
    .catch(() => undefined)
    .finally(() => {
      loading = null
    })
}

/** Detects the language, then re-registers the command in it. Never throws. */
async function localize($: EngineInterface) {
  try {
    const detected = await detectLang({
      sysname: async () => (await $.session.machine()).sysname,
      run: async argv => {
        const ran = await $.process.run(argv, { timeoutMs: 1500 })

        return ran.exitCode === 0 ? ran.stdout : ''
      },
      env: {
        LC_ALL: () => $.env.get('LC_ALL'),
        LC_MESSAGES: () => $.env.get('LC_MESSAGES'),
        LANG: () => $.env.get('LANG'),
      },
    })
    await update($, lang, () => detected)

    if (detected !== 'en') {
      await $.command.register({
        name: 'anvil-mode',
        description: STRINGS[detected].commandDescription,
      })
    }
  } catch {
    // the band stays in English
  }
}

async function anvil($: EngineInterface, args: readonly string[]) {
  const root = await $.session.root()

  return $.process.run(['anvil', '--root', root, 'orchestrator', ...args], {
    cwd: root,
    timeoutMs: 10_000,
  })
}

/** Anvil owns the mode; the mod only mirrors what `status` reports. */
async function refresh($: EngineInterface): Promise<Mode | null> {
  let next: Mode | null = null

  try {
    const ran = await anvil($, ['status', '--json'])
    next = ran.exitCode === 0 ? (JSON.parse(ran.stdout) as Mode) : null
  } catch {
    next = null
  }

  await update($, mode, () => next)

  return next
}

async function apply($: EngineInterface, args: readonly string[]) {
  try {
    const ran = await anvil($, args)

    if (ran.exitCode !== 0) {
      const t = await strings($)
      $.ui.toast(t.failed(ran.stderr.trim() || t.noChange))
    }
  } catch {
    $.ui.toast((await strings($)).unavailable)
  }

  await refresh($)
}

/** Shows the pick at once, then lets Anvil confirm or correct it. */
async function pick($: EngineInterface, next: Mode, args: readonly string[]) {
  await update($, mode, () => next)
  await apply($, args)
}

export const register: Register = on => {
  on('session.start', async ($, e, next) => {
    await $.command.register({
      name: 'anvil-mode',
      description: STRINGS.en.commandDescription,
    })
    void localize($)
    reloadCatalog($)
    void refresh($)

    return next(e)
  })

  on('command.run', { command: 'anvil-mode' }, async $ => {
    const t = await strings($)
    const now = await refresh($)

    if (!now?.initialized) {
      return { text: t.notInitialized }
    }

    await apply($, [now.enabled ? 'off' : 'on'])

    return { text: now.enabled ? t.modeOff : t.modeOn }
  })

  on('prompt.submit', async ($, e, next) => {
    reloadCatalog($)
    await refresh($)

    return next(e)
  })

  on('agent.spawn', async ($, e, next) => {
    const now = await read($, mode)
    const isExecutor = e.subagentType.split(':').pop() === 'anvil-executor'

    return now?.enabled && isExecutor && now.executor.model !== 'inherit'
      ? next({ ...e, model: now.executor.model })
      : next(e)
  })

  on('ui.render', { component: 'AbovePrompt' }, async ($, e, next) => {
    const now = await read($, mode)

    if (e.props.hasSurvey || !now?.initialized) {
      return next(e)
    }

    // Neither of these waits on detection or on the catalog: they redraw when they land.
    const t = await strings($)
    const known = (await read($, catalog)) ?? FALLBACK
    const change = (what: { model?: string; effort?: Effort }) => {
      const made = setupFor(known, now, what)
      void pick($, made.next, made.args)
    }
    const act: Actions = {
      mode: enabled => void pick($, { ...now, enabled }, [enabled ? 'on' : 'off']),
      model: model => change({ model }),
      level: effort => change({ effort }),
    }

    return band({
      ui: $.ui.resolve(e),
      t,
      catalog: known,
      now,
      act,
    })
  })
}
