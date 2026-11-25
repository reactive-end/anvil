import type { Lang } from '../types'

/** `es-US`, `es_ES.UTF-8`, `"es-419"` -> `es`; anything else -> `en`. */
export function toLang(tag: string | undefined | null): Lang | null {
  const code = /^\W*([A-Za-z]{2,3})(?:[-_.@]|\W|$)/.exec(tag ?? '')?.[1]?.toLowerCase()

  if (code === undefined || code === 'c' || code === 'posix') {
    return null
  }

  return code === 'es' ? 'es' : 'en'
}

/**
 * `reg query "HKCU\Control Panel\International\User Profile" /v Languages`
 * prints `    Languages    REG_MULTI_SZ    es-US\0es-ES\0en-US`: entries are
 * separated by a literal backslash-zero, and the first one is the preference.
 */
export function parseRegLanguages(stdout: string): string | null {
  const line = /REG_MULTI_SZ\s+(.*)$/m.exec(stdout)?.[1]?.trim()

  return line?.split('\\0')[0]?.trim() || null
}

/** `    LocaleName    REG_SZ    es-US` */
export function parseRegLocaleName(stdout: string): string | null {
  return /REG_SZ\s+(\S+)/.exec(stdout)?.[1] ?? null
}

/** `defaults read -g AppleLanguages` prints `(\n    "es-ES",\n    "en-US"\n)`. */
export function parseAppleLanguages(stdout: string): string | null {
  const body = stdout.replace(/^\s*\(/, '')

  return /^\s*"?([A-Za-z]{2,3}(?:[-_][A-Za-z0-9]+)*)"?\s*,?\s*$/m.exec(body)?.[1] ?? null
}

/** The first non-empty, non-C value of `LC_ALL`, `LC_MESSAGES`, `LANG`. */
export function firstLang(values: readonly (string | undefined)[]): Lang | null {
  for (const value of values) {
    const lang = toLang(value)

    if (lang !== null) {
      return lang
    }
  }

  return null
}
