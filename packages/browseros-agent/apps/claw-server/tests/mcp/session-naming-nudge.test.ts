import { describe, expect, it } from 'bun:test'
import type { ClientIdentity } from '../../src/lib/mcp-session'
import type { ToolCall } from '../../src/mcp/dispatch'
import { createSessionNamingEffect } from '../../src/mcp/effects/session-naming'

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

function call(value: ClientIdentity): ToolCall {
  return {
    tool: { name: 'tabs' } as never,
    args: { action: 'new' },
    sessionId: value.sessionId,
    identity: value,
    key: value.key,
    agent: { agentId: value.key, slug: value.slug },
    agentLabel: value.clientName,
    session: {} as never,
    defaultTabGroupId: null,
    flags: { newPage: true, closePage: false, listTabs: false },
  }
}

const ok = {
  content: [{ type: 'text' as const, text: 'opened page 7' }],
  isError: false,
}

describe('session naming nudge', () => {
  it('appends the prescribed tip to the first successful tabs new', () => {
    const effect = createSessionNamingEffect()
    const result = effect({
      call: call(identity()),
      result: ok,
      cancelled: false,
      durationMs: 1,
    })

    expect(result?.content).toEqual([
      {
        type: 'text',
        text: 'opened page 7\nTip: this session is "claude/swift-otter" — rename it with name_session name="<2-3 word task label>"',
      },
    ])

    expect(
      effect({
        call: call(identity()),
        result: ok,
        cancelled: false,
        durationMs: 1,
      }),
    ).toBeUndefined()
  })

  it('consumes the first successful tabs new without nudging after rename', () => {
    const effect = createSessionNamingEffect()
    const renamed = identity('invoice-processing')

    expect(
      effect({
        call: call(renamed),
        result: ok,
        cancelled: false,
        durationMs: 1,
      }),
    ).toBeUndefined()

    renamed.label = renamed.generatedLabel
    expect(
      effect({
        call: call(renamed),
        result: ok,
        cancelled: false,
        durationMs: 1,
      }),
    ).toBeUndefined()
  })

  it('does not consume the nudge on an error or a non-new call', () => {
    const effect = createSessionNamingEffect()
    const toolCall = call(identity())

    expect(
      effect({
        call: toolCall,
        result: { ...ok, isError: true },
        cancelled: false,
        durationMs: 1,
      }),
    ).toBeUndefined()
    toolCall.flags.newPage = false
    expect(
      effect({
        call: toolCall,
        result: ok,
        cancelled: false,
        durationMs: 1,
      }),
    ).toBeUndefined()

    toolCall.flags.newPage = true
    expect(
      effect({
        call: toolCall,
        result: ok,
        cancelled: false,
        durationMs: 1,
      })?.content[0],
    ).toMatchObject({ text: expect.stringContaining('Tip: this session is') })
  })
})
