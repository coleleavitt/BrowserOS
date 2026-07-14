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

/** Nudges the agent after its first successful tabs-new call. */
export function createSessionNamingEffect(): ToolEffect {
  let sawSuccessfulTabsNew = false

  return ({ call, result }) => {
    if (result.isError || !call.flags.newPage || sawSuccessfulTabsNew) {
      return undefined
    }
    sawSuccessfulTabsNew = true
    const identity = call.identity
    if (!identity || identity.label !== identity.generatedLabel)
      return undefined

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
