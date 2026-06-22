/**
 * @license
 * Copyright 2025 BrowserOS
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

import { StreamableHTTPTransport } from '@hono/mcp'
import { Hono } from 'hono'
import type { BrowserSession } from '../../browser/core/session'
import { logger } from '../../lib/logger'
import { metrics } from '../../lib/metrics'
import { Sentry } from '../../lib/sentry'
import type { KlavisService } from '../services/klavis'
import { createMcpServer } from '../services/mcp/mcp-server'
import type { Env } from '../types'

export const MANAGED_MCP_SERVERS_HEADER = 'X-BrowserOS-Managed-Mcp-Servers'
export const AGENT_ID_HEADER = 'X-BrowserOS-Agent-Id'
export const WORKING_DIR_HEADER = 'X-BrowserOS-Working-Dir'
const REMOTE_HERMES_AGENT_ID = 'remote-hermes'

interface McpRouteDeps {
  version: string
  browserSession: BrowserSession
  klavis?: KlavisService
  /** Server scratch dir (absolute). Used as the safe-default cwd for
   *  remote-hermes filesystem tools when the user hasn't picked a
   *  workspace via the side panel toolbar. */
  executionDir?: string
}

function parseOptionalNumber(value: string | undefined): number | undefined {
  if (value === undefined) return undefined
  const n = Number(value)
  // CDP window ids are integers; `Number.isFinite('1.5')` would be true
  // and silently route to a non-integer that CDP rejects with an opaque
  // protocol error. Require an integer at the parse boundary.
  return Number.isInteger(n) ? n : undefined
}

/** Parses the internal ACP managed-connector scope header. */
export function parseManagedMcpServersHeader(
  value: string | undefined,
): string[] {
  if (!value?.trim()) {
    return []
  }
  const out: string[] = []
  for (const part of value.split(',')) {
    if (!part) continue
    try {
      const decoded = decodeURIComponent(part)
      if (decoded) {
        out.push(decoded)
      }
    } catch {
      return []
    }
  }
  return out
}

export function createMcpRoutes(deps: McpRouteDeps) {
  const app = new Hono<Env>()

  app.get('/', (c) =>
    c.json({
      status: 'ok',
      message: 'MCP server is running. Use POST to interact.',
    }),
  )

  app.post('/', async (c) => {
    const scopeId = c.req.header('X-BrowserOS-Scope-Id') || 'ephemeral'
    metrics.log('mcp.request', { scopeId })

    const defaultWindowId = parseOptionalNumber(
      c.req.header('X-BrowserOS-Default-Window-Id'),
    )
    const defaultTabGroupId =
      c.req.header('X-BrowserOS-Default-Tab-Group-Id') ?? undefined
    const selectedServerNames = parseManagedMcpServersHeader(
      c.req.header(MANAGED_MCP_SERVERS_HEADER),
    )

    // Gate filesystem tools on the caller being remote-hermes — the
    // RPC dispatcher in ws-bridge stamps the agent-id header, no other
    // client does. Keeps the default MCP surface (ACP probes, external
    // MCP tools) browser-only.
    //
    // For workspace scope: prefer the user's explicit workspace
    // (header forwarded from the chat request); fall back to the
    // server scratch dir so remote-hermes always has at least a
    // sandboxed cwd. The fallback path is bounded by the server
    // process, so the LLM never escapes to the user's whole disk.
    const agentId = c.req.header(AGENT_ID_HEADER)
    const workingDir = c.req.header(WORKING_DIR_HEADER)
    const filesystemWorkingDir =
      agentId === REMOTE_HERMES_AGENT_ID
        ? workingDir || deps.executionDir
        : undefined

    // Per-request server + transport: no shared state, no race conditions,
    // no ID collisions. Required by MCP SDK 1.26.0+ security fix (GHSA-345p-7cg4-v4c7).
    const mcpServer = createMcpServer({
      version: deps.version,
      browserSession: deps.browserSession,
      klavis: deps.klavis,
      connectorScope: { selectedServerNames },
      defaultWindowId,
      defaultTabGroupId,
      filesystemWorkingDir,
    })
    const transport = new StreamableHTTPTransport({
      sessionIdGenerator: undefined,
      enableJsonResponse: true,
    })

    try {
      await mcpServer.connect(transport)
      return transport.handleRequest(c)
    } catch (error) {
      Sentry.withScope((scope) => {
        scope.setTag('route', 'mcp')
        scope.setTag('scopeId', scopeId)
        Sentry.captureException(error)
      })
      logger.error('Error handling MCP request', {
        error: error instanceof Error ? error.message : String(error),
      })

      return c.json(
        {
          jsonrpc: '2.0',
          error: { code: -32603, message: 'Internal server error' },
          id: null,
        },
        500,
      )
    }
  })

  return app
}
