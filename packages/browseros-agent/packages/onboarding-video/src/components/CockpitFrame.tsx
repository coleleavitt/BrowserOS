/**
 * @license
 * Copyright 2026 BrowserOS
 * SPDX-License-Identifier: AGPL-3.0-or-later
 *
 * The BrowserClaw cockpit chrome as it appears inside the demo. A
 * stylised recreation of the actual app: sidebar wordmark, header
 * strip, empty recent-activity table, one live-task row optional.
 * Kept SVG-simple so the render stays under budget and the visual
 * matches the app without a screenshot dependency.
 */

import type { CSSProperties } from 'react'
import { palette } from '../palette'

interface CockpitFrameProps {
  /** Optional live-task row rendered in the recent-activity table. */
  liveTask?: {
    agent: string
    action: string
    /** 0 to 1 progress. */
    progress: number
  }
  /** When true, renders a pulsing "first run will land here" dot marker in the table. */
  showLandingDot?: boolean
  /**
   * When set, wraps the named sidebar nav item in an accent-tinted
   * highlight box. Positioned inside the sidebar's flex layout so
   * the highlight cannot drift from the item across renders.
   */
  highlightNav?: 'Cockpit' | 'MCP' | 'Audit' | 'Agents'
  /** 0 to 1, scales the highlight box for a pop-in animation. */
  highlightIntensity?: number
  style?: CSSProperties
}

const RADIUS = 24

export function CockpitFrame({
  liveTask,
  showLandingDot,
  highlightNav,
  highlightIntensity = 1,
  style,
}: CockpitFrameProps) {
  return (
    <div
      style={{
        display: 'flex',
        width: '100%',
        height: '100%',
        borderRadius: RADIUS,
        overflow: 'hidden',
        boxShadow: '0 40px 80px -20px rgba(10, 13, 20, 0.35)',
        border: `1px solid ${palette.border2}`,
        background: palette.bgCanvas,
        ...style,
      }}
    >
      <Sidebar
        highlightNav={highlightNav}
        highlightIntensity={highlightIntensity}
      />
      <MainColumn liveTask={liveTask} showLandingDot={showLandingDot} />
    </div>
  )
}

function Sidebar({
  highlightNav,
  highlightIntensity,
}: {
  highlightNav?: CockpitFrameProps['highlightNav']
  highlightIntensity: number
}) {
  return (
    <div
      style={{
        width: 210,
        background: palette.bgSunken,
        padding: '32px 22px',
        display: 'flex',
        flexDirection: 'column',
        gap: 22,
        borderRight: `1px solid ${palette.border2}`,
      }}
    >
      <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
        <div
          style={{
            width: 32,
            height: 32,
            borderRadius: 9,
            background: palette.accent,
            color: palette.card,
            fontWeight: 800,
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'center',
          }}
        >
          B
        </div>
        <div
          style={{
            fontWeight: 800,
            fontSize: 15,
            letterSpacing: -0.3,
            color: palette.ink,
          }}
        >
          BrowserClaw
        </div>
      </div>
      <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
        <SidebarItem
          label="Cockpit"
          active
          highlighted={highlightNav === 'Cockpit'}
          highlightIntensity={highlightIntensity}
        />
        <SidebarItem
          label="MCP"
          highlighted={highlightNav === 'MCP'}
          highlightIntensity={highlightIntensity}
        />
        <SidebarItem
          label="Audit"
          highlighted={highlightNav === 'Audit'}
          highlightIntensity={highlightIntensity}
        />
        <SidebarItem
          label="Agents"
          highlighted={highlightNav === 'Agents'}
          highlightIntensity={highlightIntensity}
        />
      </div>
    </div>
  )
}

function SidebarItem({
  label,
  active,
  highlighted,
  highlightIntensity,
}: {
  label: string
  active?: boolean
  highlighted?: boolean
  highlightIntensity: number
}) {
  return (
    <div
      style={{
        position: 'relative',
        padding: '8px 12px',
        borderRadius: 8,
        fontSize: 13,
        color: active ? palette.ink : palette.ink3,
        background: active ? palette.card : 'transparent',
        border: active
          ? `1px solid ${palette.border2}`
          : '1px solid transparent',
        fontWeight: active ? 600 : 500,
      }}
    >
      {highlighted && (
        <div
          aria-hidden
          style={{
            position: 'absolute',
            inset: 0,
            borderRadius: 8,
            border: `2px solid ${palette.accent}`,
            boxShadow: `0 0 0 6px ${palette.accentTint}`,
            opacity: highlightIntensity,
            scale: 0.95 + highlightIntensity * 0.05,
            pointerEvents: 'none',
          }}
        />
      )}
      {label}
    </div>
  )
}

function MainColumn({
  liveTask,
  showLandingDot,
}: {
  liveTask?: CockpitFrameProps['liveTask']
  showLandingDot?: boolean
}) {
  return (
    <div
      style={{
        flex: 1,
        padding: '38px 44px',
        display: 'flex',
        flexDirection: 'column',
        gap: 28,
      }}
    >
      <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
        <div
          style={{
            fontSize: 10,
            color: palette.ink3,
            letterSpacing: 2,
            fontFamily: '"JetBrains Mono", monospace',
          }}
        >
          COCKPIT
        </div>
        <div
          style={{
            fontSize: 30,
            fontWeight: 800,
            color: palette.ink,
            letterSpacing: -0.6,
          }}
        >
          Recent activity
        </div>
      </div>
      <div
        style={{
          flex: 1,
          borderRadius: 16,
          border: `1px solid ${palette.border2}`,
          background: palette.card,
          padding: 20,
          display: 'flex',
          flexDirection: 'column',
          gap: 12,
        }}
      >
        <TableHeader />
        {liveTask ? (
          <LiveRow task={liveTask} />
        ) : showLandingDot ? (
          <LandingDotRow />
        ) : (
          <EmptyRow />
        )}
      </div>
    </div>
  )
}

function TableHeader() {
  return (
    <div
      style={{
        display: 'grid',
        gridTemplateColumns: '1fr 2fr 1fr 1fr',
        fontSize: 10,
        color: palette.ink3,
        letterSpacing: 1.6,
        fontFamily: '"JetBrains Mono", monospace',
        borderBottom: `1px solid ${palette.border2}`,
        paddingBottom: 8,
      }}
    >
      <span>AGENT</span>
      <span>ACTION</span>
      <span>STATUS</span>
      <span style={{ textAlign: 'right' }}>WHEN</span>
    </div>
  )
}

function EmptyRow() {
  return (
    <div
      style={{
        color: palette.ink3,
        fontSize: 12,
        padding: 22,
        textAlign: 'center',
      }}
    >
      Waiting for your first run.
    </div>
  )
}

function LandingDotRow() {
  return (
    <div
      style={{
        display: 'flex',
        alignItems: 'center',
        gap: 12,
        padding: '18px 8px',
      }}
    >
      <div
        style={{
          width: 10,
          height: 10,
          borderRadius: 999,
          background: palette.accent,
          boxShadow: `0 0 0 6px ${palette.accentTint}`,
        }}
      />
      <div style={{ fontSize: 12, color: palette.ink2, fontStyle: 'italic' }}>
        First run will land here.
      </div>
    </div>
  )
}

function LiveRow({
  task,
}: {
  task: NonNullable<CockpitFrameProps['liveTask']>
}) {
  return (
    <div
      style={{
        display: 'grid',
        gridTemplateColumns: '1fr 2fr 1fr 1fr',
        alignItems: 'center',
        fontSize: 13,
        padding: '10px 0',
      }}
    >
      <div
        style={{
          display: 'flex',
          alignItems: 'center',
          gap: 8,
          color: palette.ink,
          fontWeight: 600,
        }}
      >
        <div
          style={{
            width: 8,
            height: 8,
            borderRadius: 999,
            background: palette.accent,
          }}
        />
        {task.agent}
      </div>
      <div style={{ color: palette.ink2 }}>{task.action}</div>
      <div>
        <div
          style={{
            width: 120,
            height: 6,
            background: palette.bgSunken,
            borderRadius: 999,
            overflow: 'hidden',
          }}
        >
          <div
            style={{
              width: `${Math.round(task.progress * 100)}%`,
              height: '100%',
              background: palette.accent,
            }}
          />
        </div>
      </div>
      <div style={{ textAlign: 'right', color: palette.ink3 }}>just now</div>
    </div>
  )
}
