import type { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js'
import type { BrowserSession } from '../../../browser/core/session'
import {
  type BrowserToolDefaults,
  registerBrowserTools,
} from '../../../tools/browser/register'
import { registerFilesystemMcpTools } from '../../../tools/filesystem/register-mcp'

export interface RegisterToolsDeps extends BrowserToolDefaults {
  browserSession: BrowserSession
  /** When set, register filesystem tools scoped to this directory. Gated
   *  upstream by the route layer (`X-BrowserOS-Agent-Id` header) so the
   *  default MCP surface stays browser-only. */
  filesystemWorkingDir?: string
}

/** Registers BrowserOS browser tools for MCP requests. */
export function registerTools(
  mcpServer: McpServer,
  deps: RegisterToolsDeps,
): void {
  const defaults = {
    defaultWindowId: deps.defaultWindowId,
    defaultTabGroupId: deps.defaultTabGroupId,
  }

  registerBrowserTools(mcpServer, deps.browserSession, defaults)

  if (deps.filesystemWorkingDir) {
    registerFilesystemMcpTools(mcpServer, deps.filesystemWorkingDir)
  }
}
