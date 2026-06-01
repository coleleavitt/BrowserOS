/**
 * @license
 * Copyright 2025 BrowserOS
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

import { accessSync, constants, statSync } from 'node:fs'
import { join } from 'node:path'

export const BUNDLED_BUN_RELATIVE_PATH = join('bin', 'third_party', 'bun')

/** Resolves the packaged Bun executable used to run ACP adapter packages. */
export function resolveBundledBun(input: {
  resourcesDir?: string | null
  platform?: NodeJS.Platform
}): string | null {
  const platform = input.platform ?? process.platform
  if (platform !== 'darwin') return null
  const resourcesDir = input.resourcesDir?.trim()
  if (!resourcesDir) return null

  const candidate = join(resourcesDir, BUNDLED_BUN_RELATIVE_PATH)
  try {
    if (!statSync(candidate).isFile()) return null
    accessSync(candidate, constants.X_OK)
    return candidate
  } catch {
    return null
  }
}
