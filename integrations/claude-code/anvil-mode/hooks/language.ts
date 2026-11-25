import type { Lang } from '../types'
import {
  firstLang,
  parseAppleLanguages,
  parseRegLanguages,
  parseRegLocaleName,
  toLang,
} from './locale'

/** What detection needs from the engine; the closures are spelled in register.tsx. */
export type LangIo = {
  /** The OS name, or a rejection where the engine cannot say. */
  sysname: () => Promise<string>
  /** Runs a probe: its stdout, or a rejection. */
  run: (argv: readonly string[]) => Promise<string>
  env: Record<'LC_ALL' | 'LC_MESSAGES' | 'LANG', () => Promise<string | undefined>>
}

const quiet = async <T>(work: () => Promise<T>, otherwise: T): Promise<T> => {
  try {
    return await work()
  } catch {
    return otherwise
  }
}

async function fromWindows(run: (argv: readonly string[]) => Promise<string>) {
  const international = 'HKCU\\Control Panel\\International'
  const profile = await run(['reg', 'query', `${international}\\User Profile`, '/v', 'Languages'])
  const first = toLang(parseRegLanguages(profile))

  return (
    first ??
    toLang(parseRegLocaleName(await run(['reg', 'query', international, '/v', 'LocaleName'])))
  )
}

/** The machine's language, `en` when nothing answers. Never throws. */
export async function detectLang(io: LangIo): Promise<Lang> {
  const sysname = await quiet<string | undefined>(io.sysname, undefined)
  // An engine that cannot name the OS gets every probe; a missing tool reads as no output.
  const run = (argv: readonly string[]) => quiet(() => io.run(argv), '')

  const windows = sysname === undefined || sysname === 'Windows_NT' ? await fromWindows(run) : null
  const apple =
    windows === null && (sysname === undefined || sysname === 'Darwin')
      ? toLang(parseAppleLanguages(await run(['defaults', 'read', '-g', 'AppleLanguages'])))
      : null
  const detected = windows ?? apple

  if (detected !== null) {
    return detected
  }

  const values = [
    await quiet(io.env.LC_ALL, undefined),
    await quiet(io.env.LC_MESSAGES, undefined),
    await quiet(io.env.LANG, undefined),
  ]

  return firstLang(values) ?? 'en'
}
