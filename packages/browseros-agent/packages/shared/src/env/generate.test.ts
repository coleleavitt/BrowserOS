import { describe, expect, test } from 'bun:test'
import { readFileSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

import { generateEnvExample } from './generate'
import { ENV_REGISTRY } from './registry'

const ROOT_DIR = resolve(dirname(fileURLToPath(import.meta.url)), '../../../..')

describe('ENV_REGISTRY', () => {
  test('contains the root env consolidation key census in order', () => {
    expect(ENV_REGISTRY.map((spec) => spec.key)).toEqual([
      'CDP_PROTOCOL_JSON',
      'BROWSEROS_BINARY',
      'BROWSEROS_CDP_PORT',
      'BROWSEROS_SERVER_PORT',
      'BROWSEROS_EXTENSION_PORT',
      'VITE_PUBLIC_POSTHOG_KEY',
      'VITE_PUBLIC_POSTHOG_HOST',
      'VITE_PUBLIC_SENTRY_DSN',
      'VITE_PUBLIC_BROWSEROS_API',
      'VITE_ALPHA_FEATURES',
      'GRAPHQL_SCHEMA_PATH',
      'SENTRY_AUTH_TOKEN',
      'SENTRY_ORG',
      'SENTRY_PROJECT',
      'VITE_BROWSEROS_CLAW_API_URL',
      'BROWSEROS_USER_DATA_DIR',
      'BROWSEROS_CLAW_CDP_PORT',
      'BROWSERCLAW_DIR',
      'BROWSEROS_CONFIG_URL',
      'BROWSEROS_TRUSTED_ORIGINS',
      'POSTHOG_API_KEY',
      'SENTRY_DSN',
      'NODE_ENV',
      'LOG_LEVEL',
      'BROWSEROS_AI_SDK_DEVTOOLS',
      'BROWSEROS_TEST_HEADLESS',
      'OPENROUTER_API_KEY',
      'FIREWORKS_API_KEY',
      'ANTHROPIC_API_KEY',
      'OPENAI_API_KEY',
      'GOOGLE_GENERATIVE_AI_API_KEY',
      'CLAUDE_CODE_OAUTH_TOKEN',
      'EVAL_VARIANT',
      'EVAL_AGENT_PROVIDER',
      'EVAL_AGENT_MODEL',
      'EVAL_AGENT_API_KEY',
      'EVAL_AGENT_BASE_URL',
      'EVAL_AGENT_SUPPORTS_IMAGES',
      'EVAL_EXECUTOR_MODEL',
      'EVAL_EXECUTOR_API_KEY',
      'EVAL_EXECUTOR_BASE_URL',
      'CLADO_ACTION_MODEL',
      'CLADO_ACTION_API_KEY',
      'CLADO_ACTION_BASE_URL',
      'CLADO_ACTION_URL',
      'BROWSEROS_SERVER_URL',
      'BROWSEROS_SERVER_LOG_DIR',
      'NOPECHA_API_KEY',
      'WEBARENA_INFINITY_DIR',
      'INFINITY_APP_URL',
      'EVAL_R2_ACCOUNT_ID',
      'EVAL_R2_ACCESS_KEY_ID',
      'EVAL_R2_SECRET_ACCESS_KEY',
      'EVAL_R2_BUCKET',
      'EVAL_R2_CDN_BASE_URL',
      'AGENT_RUNNER_JWT_SECRET',
      'R2_ACCOUNT_ID',
      'R2_ACCESS_KEY_ID',
      'R2_SECRET_ACCESS_KEY',
      'R2_BUCKET',
    ])
    expect(ENV_REGISTRY.map((spec) => spec.key)).not.toContain(
      'R2_UPLOAD_PREFIX',
    )
    expect(ENV_REGISTRY.map((spec) => spec.key)).not.toContain(
      'R2_DOWNLOAD_PREFIX',
    )
  })
})

describe('generateEnvExample', () => {
  test('is deterministic', () => {
    expect(generateEnvExample('development')).toBe(
      generateEnvExample('development'),
    )
    expect(generateEnvExample('production')).toBe(
      generateEnvExample('production'),
    )
  })

  test.each([
    ['development', '.env.development.example'],
    ['production', '.env.production.example'],
  ] as const)('matches committed %s example', (mode, file) => {
    expect(generateEnvExample(mode)).toBe(
      readFileSync(join(ROOT_DIR, file), 'utf8'),
    )
  })
})
