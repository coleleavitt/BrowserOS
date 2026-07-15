/**
 * @license
 * Copyright 2026 BrowserOS
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

import {
  buildSessionGroupTitle,
  clientPrefixFromSlug,
} from '../../lib/mcp-session'
import type { ToolEffect } from '../dispatch'

const SESSION_NAME_NUDGE_LIMIT = 5

/** Appends bounded rename nudges while the session keeps its generated label. */
export function createSessionNamingEffect(): ToolEffect {
  let remaining = SESSION_NAME_NUDGE_LIMIT

  return ({ call, result }) => {
    const identity = call.identity
    if (
      result.isError ||
      call.tool.name === 'name_session' ||
      !identity ||
      identity.label !== identity.generatedLabel ||
      remaining === 0
    ) {
      return undefined
    }
    remaining -= 1

    const title = buildSessionGroupTitle(
      clientPrefixFromSlug(identity.slug),
      identity.label,
    )
    const tip = `Tip: this session is "${title}" — rename it with name_session name="<2-3 word task label>"`
    let appended = false
    const content = result.content.map((item) => {
      if (appended || item.type !== 'text') return item
      appended = true
      return { ...item, text: `${item.text}\n${tip}` }
    })
    if (!appended) content.push({ type: 'text', text: tip })
    return { ...result, content }
  }
}
