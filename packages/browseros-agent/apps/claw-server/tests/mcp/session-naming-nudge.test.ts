import { describe, expect, it } from 'bun:test'
import type { ClientIdentity } from '../../src/lib/mcp-session'
import type { ToolCall, ToolEffect } from '../../src/mcp/dispatch'
import { createSessionNamingEffect } from '../../src/mcp/effects/session-naming'
import type { ToolResult } from '../../src/mcp/register-fn'

const tip =
  'Tip: this session is "claude/swift-otter" — rename it with name_session name="<2-3 word task label>"'

function identity(label = 'swift-otter'): ClientIdentity {
  return {
    sessionId: 's1',
    clientName: 'Claude Code',
    clientVersion: '1.0.0',
    clientTitle: null,
    slug: 'claude-code',
    key: 'claude-code-swift-otter' as never,
    generatedLabel: 'swift-otter',
    label,
    firstSeenAt: 0,
  }
}

function call(value: ClientIdentity, toolName = 'snapshot'): ToolCall {
  return {
    tool: { name: toolName } as never,
    args: {},
    sessionId: value.sessionId,
    identity: value,
    key: value.key,
    agent: { agentId: value.key, slug: value.slug },
    agentLabel: value.clientName,
    session: {} as never,
    defaultTabGroupId: null,
    flags: { newPage: false, closePage: false, listTabs: false },
  }
}

const ok: ToolResult = {
  content: [{ type: 'text', text: 'tool result' }],
  isError: false,
}

function apply(
  effect: ToolEffect,
  toolCall: ToolCall,
  result: ToolResult = ok,
): ToolResult | undefined {
  return effect({ call: toolCall, result, cancelled: false, durationMs: 1 })
}

describe('session naming nudge', () => {
  it('appends the prescribed tip to successive arbitrary tool results', () => {
    const effect = createSessionNamingEffect()
    const value = identity()

    expect(apply(effect, call(value, 'snapshot'))?.content).toEqual([
      { type: 'text', text: `tool result\n${tip}` },
    ])
    expect(apply(effect, call(value, 'read'))?.content).toEqual([
      { type: 'text', text: `tool result\n${tip}` },
    ])
  })

  it('appends exactly five nudges then stays silent', () => {
    const effect = createSessionNamingEffect()
    const toolCall = call(identity(), 'tabs')

    for (let index = 0; index < 5; index += 1) {
      expect(apply(effect, toolCall)?.content[0]).toEqual({
        type: 'text',
        text: `tool result\n${tip}`,
      })
    }
    expect(apply(effect, toolCall)).toBeUndefined()
  })

  it('stops immediately after the session is renamed', () => {
    const effect = createSessionNamingEffect()
    const value = identity()

    expect(apply(effect, call(value))).toBeDefined()
    value.label = 'invoice-processing'
    expect(apply(effect, call(value))).toBeUndefined()
  })

  it('skips errors and name_session without consuming nudges', () => {
    const effect = createSessionNamingEffect()
    const value = identity()
    const toolCall = call(value)

    expect(apply(effect, toolCall, { ...ok, isError: true })).toBeUndefined()
    expect(apply(effect, call(value, 'name_session'))).toBeUndefined()

    for (let index = 0; index < 5; index += 1) {
      expect(apply(effect, toolCall)).toBeDefined()
    }
    expect(apply(effect, toolCall)).toBeUndefined()
  })

  it('pushes the tip when the result has no text item', () => {
    const effect = createSessionNamingEffect()
    const image = { type: 'image' as const, data: 'AAA', mimeType: 'image/png' }

    expect(
      apply(effect, call(identity(), 'screenshot'), {
        content: [image],
        isError: false,
      })?.content,
    ).toEqual([image, { type: 'text', text: tip }])
  })

  it('keeps independent counters for separate effect instances', () => {
    const first = createSessionNamingEffect()
    const second = createSessionNamingEffect()
    const toolCall = call(identity())

    for (let index = 0; index < 5; index += 1) {
      expect(apply(first, toolCall)).toBeDefined()
      expect(apply(second, toolCall)).toBeDefined()
    }
    expect(apply(first, toolCall)).toBeUndefined()
    expect(apply(second, toolCall)).toBeUndefined()
  })
})
