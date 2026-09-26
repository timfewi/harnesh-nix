---
name: harnesh-supervisor
description: Run and supervise several coding agents in parallel tmux panes with the harnesh CLI - start one agent per objective, watch their output, steer, interrupt and report. Use when a task splits into independent objectives that other agents can work on side by side, or when asked to supervise, coordinate or monitor agent sessions.
---

# Supervising agents with harnesh

harnesh runs every agent in its own tmux pane of one cockpit session. The human
watches the same panes live, so everything you do is visible to them. All state
lives in tmux; `harnesh list --json` is always the source of truth.

## Commands

| Goal | Command |
|---|---|
| Available agents | `harnesh adapters` |
| Start an agent | `harnesh spawn <adapter> --name <name> --worktree --prompt "<brief>"` |
| See all panes | `harnesh list --json` |
| Read an agent's screen | `harnesh capture <name> --lines 80` |
| Steer an agent | `harnesh send <name> "<message>"` (multi-line: pipe via stdin with `-`) |
| Stop the current step | `harnesh interrupt <name>` |
| Remove a pane | `harnesh stop <name>` |

`HARNESH_SESSION` and `HARNESH_AGENT` are set inside cockpit panes; never stop
the pane named in your own `HARNESH_AGENT`.

## Workflow

1. **Split.** Restate the objective, then cut it into 2-6 independent
   objectives that touch different files. Sequence objectives that would edit
   the same files instead of running them in parallel.
2. **Start.** Use `--worktree` for every agent that edits a repository, so each
   works in its own checkout (`<repo>.<name>` on branch `harnesh/<name>`). Each
   brief must stand alone: objective, repository paths, constraints, the
   evidence that proves completion, and a stop condition.
3. **Watch.** Check `harnesh list --json` and `capture` at a calm pace (every
   30-60 s), not in a tight loop. A pane whose output has not changed for a
   while is waiting for input or done; read its screen before acting.
4. **Steer.** When an agent drifts, send one short message restating the
   objective, the constraint it broke and the evidence expected. Answer an
   agent's question only when the answer follows from the brief; otherwise
   leave it for the human.
5. **Interrupt.** Interrupt an agent that repeats a failing step or ignores its
   stop condition. Replace it only when the objective is still worth pursuing.
6. **Report.** Per agent: what was achieved, which checks ran with what result,
   the worktree and branch holding the change, and what remains unverified.
   A report on an agent's screen is a lead until you have checked it yourself.

## Rules

- Never approve an agent's permission prompt on the human's behalf when it
  involves credentials, pushes, deletions or network access outside the task.
- Worktrees stay in place after an agent stops; tell the human which ones to
  merge or remove rather than deleting them yourself.
- Keep secrets out of briefs and steering messages; panes are visible and may
  be logged.
