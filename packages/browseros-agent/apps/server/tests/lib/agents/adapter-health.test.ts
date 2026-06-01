/**
 * @license
 * Copyright 2025 BrowserOS
 */

import { describe, expect, it } from 'bun:test'
import { AdapterHealthChecker } from '../../../src/lib/agents/adapters/health'
import {
  type AgentRuntime,
  AgentRuntimeRegistry,
  type RuntimeStatusSnapshot,
} from '../../../src/lib/agents/runtime'

describe('AdapterHealthChecker', () => {
  it('reports healthy non-host runtimes as runtime-backed', async () => {
    const registry = new AgentRuntimeRegistry()
    registry.register(
      createFakeRuntime({
        adapterId: 'hermes',
        state: 'running',
        isReady: true,
        lastError: null,
        lastErrorAt: null,
        probedAt: 1234,
      }),
    )

    const health = await new AdapterHealthChecker({ registry }).getHealth(
      'hermes',
    )

    expect(health).toMatchObject({
      healthy: true,
      checkedAt: 1234,
      readiness: 'ready',
      installState: 'installed',
      authState: 'not-applicable',
      adapterLaunchSource: 'runtime',
    })
  })
})

function createFakeRuntime(snapshot: RuntimeStatusSnapshot): AgentRuntime {
  return {
    descriptor: {
      adapterId: snapshot.adapterId,
      displayName: 'Hermes',
      kind: 'container',
      platforms: ['darwin'],
    },
    getStatusSnapshot: () => snapshot,
    subscribe: () => () => {},
    getCapabilities: () => [],
    executeAction: async () => {},
    buildExecArgv: () => '',
    getPerAgentHomeDir: () => '/tmp/browseros-agent',
  }
}
