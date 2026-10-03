---
version: alpha
name: Harnesh cockpit
description: A terminal control desk for following several coding sessions at once.
colors:
  background: "#1c1c1c"
  primary: "#dadada"
  muted: "#a8a8a8"
  selection: "#3a3a3a"
  danger: "#ff8787"
typography:
  mono:
    fontFamily: terminal-configured monospace
omitted:
  - section: rounded
    reason: Terminal cell borders use Ratatui rounded box characters.
  - section: spacing
    reason: Layout uses terminal columns and rows, documented below.
components:
  session:
    backgroundColor: "{colors.selection}"
    textColor: "{colors.primary}"
  panel:
    textColor: "{colors.muted}"
    backgroundColor: "{colors.background}"
  editor:
    textColor: "{colors.primary}"
  stop-confirmation:
    textColor: "{colors.danger}"
---

# Harnesh design system

## Overview

A fast switcher for several real agent panes, not a mirror of their terminal
output. Compact colored session rows show identity, task and observed process
status. English UI and documentation follow AGENTS.md.

This is a single-screen Ratatui TUI. The terminal owns the font. Browser, touch,
CSS, and web form contracts do not apply. Preserve the keyboard-first workflow,
agent independence, and tmux-only persistence described in README.md.

## Colors

`src/dash.rs` owns runtime tokens: BACKGROUND, INK, MUTED, SELECTION, DANGER and
HARNESS_PALETTE. This document mirrors that source (no token generation).
The five frontmatter colors correspond to xterm indices 234, 253, 248, 237, 210.
Use the dark surface consistently so identity colors remain legible.

Harness accents begin with xterm indices 81, 215, 150, 183, 221, 117, 211, 159,
179 and 111. Names deterministically select colors; collisions are resolved across
the configured adapters. The same configuration keeps its colors across restarts.
Labels always accompany color; selection also has a vertical marker and brighter
background. Danger is reserved for stopping a process. Only `running` and
`exited` describe agent status; no readiness is inferred from output. Terminal
palette overrides can change appearance.

## Typography

Bold names and task text carry hierarchy; muted text carries paths, status
details and key hints. Use terminal cell widths when clipping Unicode. Keep the
task's wrapped text in the detail panel even when its list excerpt is shortened.

## Layout

At 100 columns and above, sessions occupy 52% and metadata 48%. Below that,
stack the list above metadata. Each session is one row by default; Space expands
the selected row to three lines. The header uses three rows, the footer two.
The capacity gauge reports live agents against the configured maximum, not task
progress. Rounded borders separate surfaces without nested boxes. Session count
and selected index show where the operator is in a scrolling list.

## Elevation & Depth

No shadows. Centered help, input and confirmation overlays clear the area behind
them. Periodic refresh must not shift focus away from a selected pane.

## Shapes

One shared `panel` helper owns borders for list, detail, and overlays.

## Iconography

Text labels explain actions and states. A vertical selection marker and a small
capacity gauge provide redundant cues; no image or icon font is required.

## Motion

Refresh pane metadata once per second. Selection color changes over 210 ms and
the capacity gauge interpolates over the same interval when the running count
changes, with at most one UI-only frame every 70 ms. Both effects can be turned
off with `?`, then `a`; they never increase tmux polling or imply task progress.

## Behavior and ownership

- `Dash` owns selection, count-prefixed Vim motions, modes and feedback.
  `TableState` scrolls the list. No pane output is captured by the dashboard.
  `Enter` selects the real pane; `harnesh home` selects the dashboard pane.
- `running`/`exited` come from tmux process state; `exited` includes the exit
  code when available. These do not assert that an agent finished or is ready.
- `t` edits task metadata; Enter saves, Escape cancels, and empty text clears it.
  The editor pins the pane ID so refresh cannot redirect the saved description.
- `n` chooses an adapter and selects the new pane; `s` sends input; `i`
  interrupts; `x` explicitly confirms process termination with `y`. Send and
  stop pin the target pane ID while the modal is open. `?` explains the controls
  and offers the session-local animation toggle.
- Task summaries are normalized, bounded excerpts of supplied descriptions or
  initial prompts, never invented semantic summaries. `Tmux::set_task` and
  `task_summary` own this format; only `@harnesh_task` persists it.
- Empty sessions show a three-step quick start; missing task text invites `t`.
  Errors remain in the footer. Configuration stays in config.rs.

## Verification

Ratatui TestBackend checks wide/narrow layouts, tiny terminal resilience,
compact rows, navigation, modal states and Unicode. Real tmux integration
tests check switching, no mirrored output, task editing/cancellation and
rendering without a UTF-8 locale. Run project-check fast and full in the Nix
development shell. Browser-oriented audits cannot validate a terminal UI.
