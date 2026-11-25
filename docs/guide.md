# Getting started

English | [简体中文](guide.zh.md)

Sign in at your installation's address to ask a question, delegate work or follow up across devices. [Using the main conversation](design/main-entry.md)

## 1. Install

| Use | Get started |
|---|---|
| Hosted Web and iOS PWA | Open your deployment's HTTPS address and sign in or register. Wait for the workspace to start on first use. |
| Self-hosted on one computer | Run `just local-up` from the checkout and create the owner account in the browser. |
| Mac app | Approve device pairing in the browser for the hosted app. The development app opens the same Go gateway. See [Mac desktop app](operations.md#mac-desktop-app). |

## 2. The first-run wizard

A personal deployment opens the wizard after sign-in when no model is connected:

1. **Connect a model.** Pick a provider and paste its key. The key goes to the system keychain; Ollama needs no key.
2. **Let Jeeves get to know you.** Your name and what you care about lately are saved as memory you can open and edit.

Then say something. A good first message: "Summarize the files in my Downloads folder and propose how to tidy them."

## 3. Models

Use `/model` in the Jeeves CLI to manage profiles. Changes take effect on the next request. A profile is a provider, a model name and a key source. Keys go to the OS keychain, falling back to `<home>/secrets.json` (0600, plaintext) when no keychain is usable; a profile can instead name an environment variable (`env:VAR`), which is read but never copied. The save response says where the key landed.

```
/model                                       list profiles (★ = active)
/model add ds deepseek deepseek-flash --env DEEPSEEK_API_KEY
/model use ds                                switch
```

Declare image support when you add a profile (`--vision` / `--no-vision`), otherwise the vendor's default row decides. For local inference, install Ollama, [pull the default models](configuration.md#local-inference-ollama) and add an `ollama` profile. See the [configuration reference](configuration.md#model-profiles-and-their-keys-homemodelsjson) for model routing and profile options.

## 4. Permissions

Review the action, working directory and preview on the approval card in the conversation, then allow it once or deny it. When answering one card after another gets tiresome, the card also offers **Stop asking — I trust this session**: it confirms once, then allows the card in front of you and puts the session into full access, so reading and writing files and running commands no longer ask. Sending something out, which cannot be taken back, still asks every time, and a card for a send never offers the switch. The switch is a human control — Jeeves can neither press it nor raise its own permissions. The operator manages the Runtime switches for commands, file writes and delegation. See the [protocol](protocol.md) for session modes and [security](../SECURITY.md) for sandbox boundaries.

## 5. Channels: Lark and Telegram

Both gateways talk to a Jeeves Runtime and derive session ids from the platform identity.

### Connection status

Browse the catalog for provider documentation or add a custom MCP connection. Remote connections take a URL and support anonymous access, a bearer environment variable or browser OAuth. Local stdio connections take a command and arguments. Entries requiring provider setup link to the provider’s instructions. Open tool permissions on an MCP card to inspect effects or disable and restore tools on your own connection. Changes take effect on the next turn. Re-enabled tools retain their approval requirements.

Open **Connections** in Settings to see the configured capabilities together, with those needing attention first. Cards show local status, credential location (never a value), last check time and next steps. Checks may use the 60-second cache and do not verify remote service acceptance. Under **Local connectors**, add or edit Calendar and Email; save secrets separately in the credential vault and reference `vault:NAME` or `env:VAR`. For configured OAuth-capable MCP or Feishu user connections, choose **Start authorization** and open the returned link on the device running Jeeves, where the loopback callback listens. Complete the provider's consent screen and recheck; a CLI fallback is provided if preparation fails. No QR flow is fabricated. See [Capability cards](protocol.md#capability-cards) for the wire states.

### Lark / Feishu

Lark uses a long connection, so no public address is needed. In the developer console create a self-built app and note the App ID and App Secret, then:

1. Enable **bot capability**.
2. Subscribe to `im.message.receive_v1`.
3. Choose the **long connection** delivery method, not "push to your server".
4. For approval cards, also subscribe to the callback `card.action.trigger`.

Missing any of the first three produces "connected, forever silent" with no error. Then open Menu → **Connections**, enter the App ID and App Secret (the secret goes to the keychain), and turn the gateway on. The Runtime starts it, shows its state and last log lines, and restarts it with the Runtime; Node.js must be installed. Leave the allowlist empty and the gateway answers nobody: the first person who writes appears as a candidate, and one click confirms them as the owner. For the international Lark, set the domain to Lark.

Allowlist entries are **open_ids** (`ou_…`), not App IDs. If logs say `outside the allowlist`, the sender's open_id did not match; open_ids differ between applications. In groups the bot only answers when mentioned.

For a bot direct message or an ordinary group turn from the sole allowlisted owner, Jeeves replies with an approval card showing the operation and time limit. The original sender can use the buttons or send the approval text shown in the card. Jeeves sends the approval as text if card delivery fails. For text approval in a group, mention the bot in the reply. An allowlisted dispatcher can use the same flow in direct messages. If the request expires or the gateway restarts, check the operation's status before sending it again.

Without the dashboard, run `just gateway-lark` with `LARK_APP_ID`, `LARK_APP_SECRET` and `LARK_ALLOW` set; `JEEVES_GATEWAY_URL` (plus the same `JEEVES_OPS_TOKEN`) attaches it to a running Runtime instead of starting its own engine. The variables are in the [configuration reference](configuration.md#channel-adapters).

#### Dispatching tasks from a group

List the people who may hand out work in `JEEVES_DISPATCHERS` on the Runtime (or in the Connections screen). When one of them @-mentions the bot in a group, Jeeves may create a task that remembers the group, the message and the sender. The group hears the receipt and the final report; progress stays on the task board. Anyone else still gets an answer, in a separate conversation without the `task` tool and without the owner's memory. The Runtime checks the list itself, so the gateway cannot widen it.

When a dispatched turn needs approval, the gateway posts an interactive card in the thread. Only a listed dispatcher's click counts, and it answers once; a chat can never grant a standing rule.

#### Feishu business tasks through the official CLI

The gateway carries conversations only. For Feishu business APIs, use the [official lark-cli](https://github.com/larksuite/cli) through `run_command` and the bundled `feishu-cli` skill:

```sh
just install-feishu-cli                     # pinned CLI under .jeeves/tools/lark-cli
just feishu-cli auth status                 # reuse the CLI's own login
export JEEVES_SKILL_DIRS="$PWD/skills"
export JEEVES_EXEC=1 JEEVES_EXEC_NETWORK=1 JEEVES_APPROVAL=on-request
```

Restart the Runtime with these exported. Unmatched commands ask for approval; do not grant full access just for the CLI. The CLI keeps its own account and credentials, independent of `LARK_*` and `connectors.json`. If a token refresh needs a writable cache directory, add only that directory to `JEEVES_EXEC_WRITABLE_ROOTS`.

### Telegram

Telegram uses long polling, so it needs no public address. Make a bot with @BotFather, then:

```sh
TELEGRAM_BOT_TOKEN=… TELEGRAM_ALLOW=<your numeric user id> just gateway-telegram
```

The token is read only from the environment. `TELEGRAM_ALLOW` lists numeric user ids; unset means anyone who finds the bot can use it. `TELEGRAM_CLAIM=1` answers nobody and logs the first sender as a candidate. Only direct chats are answered. An approval arrives as a message with **Approve once** / **Deny** buttons, sent to every listed user; the Runtime accepts the first answer and refuses stale ones. Button keys are kept in memory, so an approval asked before a gateway restart can no longer be answered from its old buttons. The update offset is saved in `<home>/telegram-offset`. This gateway has not been run against the real Telegram service.

## 6. Memory

Jeeves keeps semantic facts, dated episodes and skills, and uses local keyword search to retrieve matching memory during a full turn. You can inspect all of it: the **Memory** tab in Settings shows the **Ledger** (one row per turn; open it for that turn's entry stream), the **Library** (a catalog and one page at a time, with `ledger:…` chips back to the source turns) and the **Context** meter (what grows with the session, with a notice at `JEEVES_CONTEXT_WARN_RATIO`). **Compact now** folds older turns into the rolling summary on demand.

The library is plain text under `<home>/memory`: edit a page by hand and `jeeves memory rebuild` takes your edit back into the facts and regenerates the rest; `jeeves memory backfill` rebuilds ledger rows from old chat history. Other agents on this machine can share the same memory through `jeeves mcp serve` ([operations](operations.md#serving-jeevess-memory-to-other-agents)).

## 7. Skills

A skill is procedural memory: one `SKILL.md` describing **how** to do a class of task, loaded when a relevant message appears. A new home gets two starter skills once: `summarize-document` and `tidy-folder`. They are plain files in `<home>/skills/`; edit or delete them and they stay that way.

The repository's `skills/` folder carries optional extras, loaded with `export JEEVES_SKILL_DIRS="$PWD/skills"`. Among them, `dashboard-verification` is the check list for changing the dashboard: it keeps a function map of what to prove, the commands that prove it, and the evidence to keep. Use it before calling a dashboard change done.

```markdown
---
name: weekly-review
description: Summarize the week and draft the Monday brief
---

1. Pull last week's episodes from memory.
2. Distill into three bullets.
3. Draft the digest into the outbox.
```

### How a skill is chosen

Every turn carries a one-line-per-skill catalog (capped by `JEEVES_SKILL_INDEX_TOKENS`); a skill's body enters the prompt only when needed: you name it as `$weekly-review`, its name and description share at least two distinct words with your message (ASCII words of three or more characters, or adjacent Han-character pairs), or the model reads it from the catalog with `read_page` (one body is capped by `JEEVES_SKILL_BODY_TOKENS`). So write the `description` as the trigger. Files a skill references are read only when the model asks. Hits are appended to `<home>/skill-hits.jsonl` and summarized under **Skills** in the dashboard.

Two optional frontmatter fields set policy: `allow-model-invocation: false` stops a skill from firing on keyword overlap (it loads only when named with `$name`), and `dependencies: a, b` lists skills that must be present, otherwise it does not fire implicitly. `version: 1.2.0` enables updates, and `schedule: 0 8 * * 1` makes the skill a [scheduled job](#8-schedules). Describe the process, never borrow tools: a skill exported to another agent cannot use Jeeves's built-in tools.

### Installing, updating and sharing

```bash
jeeves skill list                                    # what is loaded
jeeves skill install <SKILL.md url or local path>    # validated; never overwrites
jeeves skill update [index.json or url]              # newer versions only; default <home>/skills/index.json
jeeves skill export --to claude,codex                # copy to other agents (--project, --force, --names a,b)
```

Update validates first, swaps atomically, keeps the old copy in `.backup/` and never downgrades. The layout is `<home>/skills/<name>/SKILL.md` with a lowercase slug name; on a name clash `<home>/skills` wins over `JEEVES_SKILL_DIRS`. After teaching Jeeves a flow in conversation, say "save this as a skill" and it calls `create_skill`. The approval card shows the full SKILL.md draft; a denial, a timeout or a turn with nobody to answer leaves the disk untouched. An existing skill is never overwritten.

The dashboard's **Capabilities** center lists every loaded skill with version, policy and missing dependencies, and lets you add or delete the ones in `<home>/skills/`. Skills from `JEEVES_SKILL_DIRS` are built in and read-only. Installs take effect on the next turn.

## 8. Schedules

Ask in chat, “At 8 each weekday, send my calendar for the day here.” Review the time, source and destination before activating it. Amend or pause it from the same conversation. [Recurring work](design/main-entry.md#6-recurring-work)

## 9. Credentials

Menu → **Connections** → **Credentials** stores a password or token under an uppercase name (letters, digits, underscore). Jeeves's commands can use it but never read it: the model writes `$JEEVES_SECRET_GITHUB_TOKEN` in a command, the Runtime injects the value into that one process and scrubs it from the output. Values live in the keychain; listing shows names only. This covers `run_command` only; see [limitations](limitations.md#credential-vault-covers-commands-only).

## 10. Inbox

Settings → **Inbox** is one read-only list of everything waiting on you: tasks that need your call, and drafts Jeeves left in `<home>/outbox/` (briefings, messages it was asked to write but never sends). Approvals keep their own cards in the chat.

An outbox that has not been created yet is empty. If Jeeves cannot read an existing outbox, the panel shows the error and offers a refresh.

## 11. Browser boundaries

The task browser starts clean and has no saved site credentials. Jeeves does not solve CAPTCHAs,
bypass bot checks, or store passwords in the browser. When a site needs a sign-in, 2FA, passkey or
payment confirmation, hand control back with the task card's takeover; the person finishes that step
and returns the browser. Sensitive fields, popups, uploads and downloads always need that help.
An optional [Browse connector](configuration.md#built-in-mcp-servers-jeeves_mcp_config) can reuse an
already-signed-in browser; it is a transport, not a way around a challenge, and it does not add the
task browser's generation fencing or frame-based takeover.

## 12. Moving a home between local and cloud

Everything Jeeves knows is under one folder, its *home* (`./.jeeves` by default, or `JEEVES_HOME`): database, memory, skills, settings. Move it with:

```bash
jeeves home export <dir>      # into a new, empty directory; safe while Jeeves runs
jeeves home import <dir>      # into an empty JEEVES_HOME; older schemas are migrated
```

The same pair moves a home between machines and between a local install and a hosted Pod. Keys and sign-ins are not exported (`secrets.json`, `mcp-auth/`, `connector-auth/`); add them again after importing. For hosting many users, see [operations](operations.md#running-jeeves-for-many-users).

## Going further

[Configuration reference](configuration.md) (for operators) · [Operations](operations.md) · [Security](../SECURITY.md) · [Limitations](limitations.md)
