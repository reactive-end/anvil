import type { Catalog } from '../types'
import { newestCatalog } from './catalog'

const FILES_READ = 12

export type CatalogIo = {
  env: Record<'CLAUDE_CONFIG_DIR' | 'USERPROFILE' | 'HOME', () => Promise<string | undefined>>
  list: (dir: string) => Promise<readonly { name: string; kind: string; mtimeMs: number }[]>
  read: (path: string) => Promise<string>
}

/**
 * The desktop app's picker cache: only the `*-ccd.json` files of its own
 * `model-catalog` folder are listed and read (the folder also holds
 * `published-floor.json`, which is not a catalog). Null when nothing usable
 * is there; never throws.
 */
export async function loadCatalog(io: CatalogIo): Promise<Catalog | null> {
  try {
    const configured = await io.env.CLAUDE_CONFIG_DIR()
    const home = (await io.env.USERPROFILE()) ?? (await io.env.HOME())
    const base = configured ?? (home === undefined ? undefined : `${home}/.claude`)

    if (base === undefined) {
      return null
    }

    const dir = `${base}/cache/model-catalog`
    const files = (await io.list(dir))
      .filter(entry => entry.kind === 'file' && entry.name.endsWith('-ccd.json'))
      .sort((a, b) => b.mtimeMs - a.mtimeMs)
      .slice(0, FILES_READ)
    const texts: string[] = []

    for (const file of files) {
      try {
        texts.push(await io.read(`${dir}/${file.name}`))
      } catch {
        // an unreadable file is skipped
      }
    }

    return newestCatalog(texts)
  } catch {
    return null
  }
}
