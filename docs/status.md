# Session status

Code: `src/claude.rs` (`read`), `src/ui.rs` (`detect`), `src/ui/chrome.rs` (rail cards).

punkterm reads each Claude Code session's state off its screen, the way
cc-rail's collector does, and shows it as the card's chip and activity line:

| Status | Chip | When |
|---|---|---|
| Blocked | `INPUT` (pulsing) | A permission prompt ("Do you want", "❯ 1. Yes") near the bottom |
| Working | `RUN` + spinner | The spinner line ("… (12s", "esc to interrupt"), or its own agents or workflows running in the background |
| Your turn | `READY` | Was working, now isn't, and you haven't typed since |
| Idle | `IDLE` | A Claude session doing nothing |
| Shell | none | Not a Claude session |

## Background work

A session's own background work is read from its screen (`claude::background`)
and summarised on its card, e.g. `4 agents · 3 workflows · 8 shells`:

- **Task panel** (under the footer, while tasks run): `● main`, then one
  `◯ name  doing…` row per running agent and `◯ name  ▰▰▱▱ 1/2 · …` per
  workflow (the rows with a progress bar).
- Failing that, the live status line just above the prompt:
  `✻ Waiting for 1 background agent and 3 dynamic workflows to finish`
  (`1 agent · 3 workflows`). Earlier `Waiting for` lines left in the
  transcript are ignored.
- **Footer counts** between `·` separators: `8 shells`.

Agents and workflows keep the session **Working** after its own turn ends;
its activity line is the summary, or `Pondering… · 4 agents · 3 workflows`
while the turn is busy too. Shells alone don't (one may be a server left
running): they are added to the card's line instead, `idle · 5m · 1 shell`.

The footer's `← 4 agents` is every agent on the host (`claude agents`), the
same in every session there, so it is ignored.
