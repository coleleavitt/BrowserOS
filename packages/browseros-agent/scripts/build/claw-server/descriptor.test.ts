import { afterEach, beforeEach, describe, expect, it } from 'bun:test'
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { loadBuildConfig } from '@browseros/build-server-tools'

import { clawServerBuildProduct } from './descriptor'

describe('claw server build descriptor', () => {
  let tempRoot: string | null = null
  let originalNodeEnv: string | undefined

  beforeEach(() => {
    originalNodeEnv = process.env.NODE_ENV
    delete process.env.NODE_ENV
  })

  afterEach(async () => {
    if (originalNodeEnv === undefined) {
      delete process.env.NODE_ENV
    } else {
      process.env.NODE_ENV = originalNodeEnv
    }

    if (tempRoot) {
      await rm(tempRoot, { recursive: true, force: true })
      tempRoot = null
    }
  })

  it('inlines NODE_ENV from the production env file', async () => {
    const rootDir = await writeClawPackageRoot('NODE_ENV=production\n')

    const config = loadBuildConfig(rootDir, clawServerBuildProduct)

    expect(config.envVars.NODE_ENV).toBe('production')
  })

  it('defaults CI builds to production NODE_ENV without an env file', async () => {
    const rootDir = await writeClawPackageRoot()

    const config = loadBuildConfig(rootDir, clawServerBuildProduct, {
      ci: true,
    })

    expect(config.envVars.NODE_ENV).toBe('production')
  })

  it('forces CI builds to production NODE_ENV over ambient env', async () => {
    process.env.NODE_ENV = 'development'
    const rootDir = await writeClawPackageRoot()

    const config = loadBuildConfig(rootDir, clawServerBuildProduct, {
      ci: true,
    })

    expect(config.envVars.NODE_ENV).toBe('production')
  })

  async function writeClawPackageRoot(envContent?: string): Promise<string> {
    tempRoot = await mkdtemp(join(tmpdir(), 'claw-server-build-descriptor-'))
    const packageDir = join(tempRoot, 'apps/claw-server')
    await mkdir(packageDir, { recursive: true })
    await writeFile(
      join(packageDir, 'package.json'),
      '{"version":"0.0.0-test"}',
    )
    if (envContent !== undefined) {
      await writeFile(join(tempRoot, '.env.production'), envContent)
    }
    return tempRoot
  }
})
