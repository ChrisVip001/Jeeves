---
name: morning-briefing
description: Write the morning briefing draft. Scan GitHub (open PRs and issues), the web, today's calendar and memory, then synthesize one short proposal for the day. Propose only: never act, never send.
---

# Morning briefing

Produce one short draft for the person to read over coffee. This is a **propose-only** task: read, summarize,
suggest. Never write outside the outbox, never run commands that change anything, never send a message.

## Scan (do these in parallel when you can)

1. **GitHub**: if `gh` is available and a repository is named in the task or in memory, list open PRs and issues
   (`gh -R <repo> pr list --state open --limit 20`, same for issues). If it is not configured, write
   "GitHub: not configured" so the reader can tell "not set up" from "nothing going on".
2. **Web**: `search_web` for the topics named in the task (or memory). Keep two or three items that matter.
3. **Calendar**: `glob`/`read_file` on `<home>/calendar.ics` for today's events. No file means "no calendar".
4. **Memory**: `memory(action=search)` for the project, repo and release context.

A source that fails becomes one honest line ("web: unavailable, <reason>"). It never stops the briefing.

## Synthesize

Write a briefing of at most 15 lines: what changed, what needs a decision today, what can wait. Order by what the
person must act on. Say which items are suggestions.

## Deliver

Save the draft with `send_message` (it only writes a draft into `<home>/outbox/`). Reply with the same text.

To run it every morning, ask Jeeves: "run the morning-briefing skill every weekday at 8" (a `schedule_job`), or add a
`schedule: 0 8 * * 1-5` line to this skill's frontmatter in your own skills directory.
