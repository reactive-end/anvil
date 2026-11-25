import { expect, test } from 'claude-code/testing'

import { STRINGS, effortName, modelName } from './i18n'
import {
  firstLang,
  parseAppleLanguages,
  parseRegLanguages,
  parseRegLocaleName,
  toLang,
} from './locale'

// Verbatim from `reg query` on Windows 11 (stdout, blank lines kept).
const LANGUAGES = [
  '',
  'HKEY_CURRENT_USER\\Control Panel\\International\\User Profile',
  '    Languages    REG_MULTI_SZ    es-US\\0es-ES\\0en-US',
  '',
].join('\r\n')

const LOCALE_NAME = [
  '',
  'HKEY_CURRENT_USER\\Control Panel\\International',
  '    LocaleName    REG_SZ    es-US',
  '',
].join('\r\n')

test('reg Languages: first entry of the \\0 separated list', async () => {
  expect(parseRegLanguages(LANGUAGES)).toBe('es-US')
  expect(parseRegLanguages('    Languages    REG_MULTI_SZ    en-GB')).toBe('en-GB')
  expect(parseRegLanguages('')).toBeNull()
  expect(toLang(parseRegLanguages(LANGUAGES))).toBe('es')
})

test('reg LocaleName: the REG_SZ value', async () => {
  expect(parseRegLocaleName(LOCALE_NAME)).toBe('es-US')
  expect(parseRegLocaleName('ERROR: not found')).toBeNull()
})

test('defaults AppleLanguages: first quoted entry', async () => {
  expect(parseAppleLanguages('(\n    "es-ES",\n    "en-US"\n)\n')).toBe('es-ES')
  expect(parseAppleLanguages('(\n    en\n)')).toBe('en')
  expect(parseAppleLanguages('')).toBeNull()
})

test('es-* is Spanish, everything else English, C and POSIX undecided', async () => {
  expect(toLang('es-419')).toBe('es')
  expect(toLang('es_CL.UTF-8')).toBe('es')
  expect(toLang('pt-BR')).toBe('en')
  expect(toLang('C')).toBeNull()
  expect(toLang('POSIX')).toBeNull()
  expect(toLang('')).toBeNull()
  expect(firstLang([undefined, '', 'C', 'es_ES.UTF-8', 'en_US'])).toBe('es')
})

test('model and effort names', async () => {
  const en = STRINGS.en
  const es = STRINGS.es
  expect(modelName('opus', en)).toBe('Opus')
  expect(modelName('claude-sonnet-4-5', es)).toBe('claude-sonnet-4-5')
  expect(modelName('inherit', es)).toBe('El de la sesión')
  expect(modelName('inherit', en)).toBe('Session model')
  expect(effortName('xhigh', es)).toBe('Extra')
  expect(effortName('max', es)).toBe('Máx')
  expect(effortName('max', en)).toBe('Max')
})
