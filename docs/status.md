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

## Host load

The rail's footer shows this machine's CPU, memory and disk use (the root
filesystem, `/`, as `df` counts it: used over used plus available). While the
session on screen is on a remote host, that host's readings are stacked
below, under its name: sampled every 3 s over the shared ssh connection
(`sysstat::remote`: `/proc` and `df -Pk /`, so Linux hosts), and dropped
when a reading is older than 20 s. This machine's disk is resampled every
30 s.

## Accounts and plan usage

A host can run several Claude Code accounts at once, each Claude started with
its own config folder (`CLAUDE_CONFIG_DIR=~/.claude-dev1 claude`; the default
is `~/.claude`). punkterm reads, on each host, every `~/.claude*/.claude.json`
and which config each pane's Claude runs (from the process's environment):

- **Cards** on such a host show the session's account (`dev1`, `alice`)
  before the status chip.
- The rail's **PLAN** panel lists every account config on the active
  session's host: its own account first (●) with its limits' bars and how
  old the reading is, each other signed-in account on one line
  (`dev1  5H 93%  WK 40%`), and the configs not signed in yet together on
  one line. Click **PLAN** to fold the active one to a line too, again to
  unfold; **ALL ›** opens the usage overview.
- The **LOAD** panel below (CPU, MEM, DISK for this machine and the session's
  remote host) folds to one line per host the same way, by its header.
  Both stay folded or open across restarts.

Usage comes from Claude Code's own cache in each config (`src/usage.rs`,
every minute). Right after a `/login` with another account that cache still
holds the previous account's numbers until Claude Code reads the new one's.
The cache names its account (`accountUuid`), so punkterm uses it only when it
names the one signed in, and says WAITING until then rather than show someone
else's usage. cc-rail's copy (keyed by config file) names no account; it
counts only when newer than a cache that matches.

**Ctrl+Shift+U**, or **ALL ›**, opens the usage overview. It lists:

- every account signed in on one of your hosts now, with those hosts, each
  limit's meter, percentage and time to reset, and how old the reading is
  (Claude Code refreshes it only while its sessions make requests); one
  waiting on Claude Code after a login says so, over its last kept reading;
- every other account punkterm has seen, since a host forgets an account
  once it is logged out: "signed out · seen 2h ago", with where it was and
  its last reading, dimmed. A window whose reset time has passed shows RESET.

Accounts are grouped under their email's domain, alphabetically within it;
on a wide window the domains share two columns. punkterm keeps the accounts
it has seen in `~/.local/state/punkterm/accounts.toml`. Opening the overview
reads every host again. Esc, U or a click closes it.
