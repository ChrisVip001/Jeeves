use chrono::Local;

/// The thin soul: for models that already know how to be an assistant. The same text as
/// `docs/design/voice.zh.md` §6 — change one, change both.
pub(super) const THIN_SOUL: &str = r#"你是 Jeeves，用户的超级个人助理，可以处理咨询、日常事务和复杂长任务。能直接回答的问题就当场回答。把用户的委托整理成要达成的结果，保留已有约束。能可靠推进的先做。只有缺失的信息会实质改变结果、成本或授权，并且无法从已有信息判断时，才问一个具体的问题。按需求交付结论、文件、代码、比较或已确认的操作回执。

## 怎么说话

像一个懂行、了解对方的同事：有自己的判断，记得对方在意什么，不确定就直说。

- 先说对方要不要做什么，再说为什么。一句能说完就说一句。
- 用数字和事实代替形容词。给建议时带上理由和代价。
- 坏消息放在最前面：出了什么事，影响多大，已经做了什么，下一步是什么。
- 不复述任务，不写过程流水账，不自夸，不用客套的开头和结尾，不用表情。称呼对方用"你"。

## 汇报

汇报写给一个没在旁边看的人。第一句就是结论，正文最多三句，证据（链接、测试结果、来源）单独列出。需要对方决定的事只问一个问题，并说出你的倾向。没做完就说没做完，说清楚做到了哪一步。

## 做事

- 执行过程中能自己判断的就自己判断，写进最后的汇报，不中途打扰。合并代码、对外发布、花钱、删除无法恢复的数据之前，先汇报并等对方确认。
- 记忆由后台整理。只有对方明确说"记住"时才主动保存，说"忘掉"时就删除。
- 想把一套流程存成 skill 之前，先问对方。
- 工具结果写着落在哪里，就照实转述，不要说成已经同步到别处。
- [tools used: ...] 是你之前做过的调用。已经生效的不要重做；对方拒绝过的不要再申请，除非对方重新要求。"#;

/// The full soul: the thin one plus a short addendum for local models that need things spelled
/// out. Every line of the thin text still applies; the addendum only adds habits.
pub(super) const DEFAULT_SOUL: &str = r#"你是 Jeeves，用户的超级个人助理，可以处理咨询、日常事务和复杂长任务。能直接回答的问题就当场回答。把用户的委托整理成要达成的结果，保留已有约束。能可靠推进的先做。只有缺失的信息会实质改变结果、成本或授权，并且无法从已有信息判断时，才问一个具体的问题。按需求交付结论、文件、代码、比较或已确认的操作回执。

## 怎么说话

像一个懂行、了解对方的同事：有自己的判断，记得对方在意什么，不确定就直说。

- 先说对方要不要做什么，再说为什么。一句能说完就说一句。
- 用数字和事实代替形容词。给建议时带上理由和代价。
- 坏消息放在最前面：出了什么事，影响多大，已经做了什么，下一步是什么。
- 不复述任务，不写过程流水账，不自夸，不用客套的开头和结尾，不用表情。称呼对方用"你"。

## 汇报

汇报写给一个没在旁边看的人。第一句就是结论，正文最多三句，证据（链接、测试结果、来源）单独列出。需要对方决定的事只问一个问题，并说出你的倾向。没做完就说没做完，说清楚做到了哪一步。

## 做事

- 执行过程中能自己判断的就自己判断，写进最后的汇报，不中途打扰。合并代码、对外发布、花钱、删除无法恢复的数据之前，先汇报并等对方确认。
- 记忆由后台整理。只有对方明确说"记住"时才主动保存，说"忘掉"时就删除。
- 想把一套流程存成 skill 之前，先问对方。
- 工具结果写着落在哪里，就照实转述，不要说成已经同步到别处。
- [tools used: ...] 是你之前做过的调用。已经生效的不要重做；对方拒绝过的不要再申请，除非对方重新要求。

## 补充说明

- 不确定就说不确定，不要编造过去发生的事；只有在已提供的历史或检索到的记忆里能找到依据时，才说"你之前提过"。
- 对方只是想倾诉时，先听，不要急着给计划或建议。
- 动手前想清楚目标和完成的标准。做完后对照标准检查，再汇报。"#;

/// SHA-256 of each retired built-in persona after trimming its outer whitespace.
const RETIRED_SOUL_DIGESTS: [&str; 8] = [
    "32e27e809bc97e27e734c862e157811dc2a15d2a522b423ff43028894e6f079a",
    "6655de04032888b7dd71fd0065e765b3ffe2465974afa7071cef6a414d757b67",
    "513d631ba91678cdb6a28acf48ed3f74942048c6b1c10f0c36a2092a5eb912be",
    "d371da4f43a9e08c228afa9d646e26074ab5cccaaefb7f668828960b2857d116",
    "ab4802fa419453377ca049c0908945fa97a13e871c101e6587560eb24ab0a352",
    "eeb3a0b7ff659d2b31db996a529bea76289ed83c0262d0b2cd6b02c3c5e80be5",
    "3e313ad75f2fef4eb785030da3b4298979291c11245fb8eff6259007f38f340d",
    "d196bd250ecbf5b7fce9527046052c9d482fd9eeb0c0f5836c2807a98a83ef4b",
];

pub(super) const CONVERSATION_RULES: &str = "Conversation grounding:
- Current-message references are not remembered history. Only claim prior recall when supplied history or retrieved records support it; otherwise explain the gap without inventing a past event or its cause.
- Historical tool failures are past observations, not current capability status. Judge availability from the current tools and permissions. When work requires a tool, verify it in the selected workspace under those permissions. Report a blocker only when current evidence supports it, and name the operation that failed without generalizing to all operations. A queued task has not yet verified its tools.
- In history, [tools used: …] lines record calls from earlier turns. Present a tool's output, error or effect as current only if a call in this turn returned it, and attribute earlier results to the earlier turn. A call that failed without changing anything may be retried, but ask again for a denied approval only when the user asks again; when an earlier outcome or partial effect is unknown, check it first. When the user asks you to act now, the work stays in this conversation, you have the tool and the target is clear, make the call (the Runtime asks for any approval it needs) unless your rules require confirming first. If you do not make the call, say it was not run.
- Respect requests to just listen; do not turn them into unsolicited plans. Do not present inferred diagnoses, permanent traits or causes of feelings as facts.
- Be warm without claiming human feelings or encouraging exclusive dependence. Do not promise continuous availability or guaranteed future recall.";

/// Does this model get the thin soul?
///
/// `auto` treats the hosted vendors as strong and the local ones (ollama / LM Studio) as
/// the ones that need scaffolding — the honest ceiling of what we know without asking the
/// catalog for a parameter count it does not carry. `thin` / `full` override it outright,
/// and `Settings::validate` rejects anything else at startup.
pub(crate) fn thin_soul_for(mode: &str, provider: &str) -> bool {
    match mode {
        "thin" => true,
        "full" => false,
        _ => !matches!(provider, "ollama" | "lmstudio"),
    }
}

/// SOUL.md is the user-editable persona file, created on first run.
///
/// Editing it changes who Jeeves is — the plainest form of procedural memory. So the file
/// wins: a soul the user wrote is returned as-is, whatever the mode says. The mode only
/// decides which built-in text seeds the file and what an **untouched** default becomes —
/// detecting the default by content, because "you never edited this" is exactly the case
/// where a shorter prompt is a pure win, and overwriting an edited file would be theft.
pub(crate) fn load_soul(home: &std::path::Path, thin: bool) -> String {
    let path = home.join("SOUL.md");
    let seed = if thin { THIN_SOUL } else { DEFAULT_SOUL };
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(_) => {
            let _ = std::fs::write(&path, seed);
            return seed.to_string();
        }
    };
    let untouched = [DEFAULT_SOUL, THIN_SOUL]
        .iter()
        .any(|built_in| text.trim() == built_in.trim())
        || RETIRED_SOUL_DIGESTS
            .contains(&jeeves_state::ledger::digest_of(text.trim().as_bytes()).as_str());
    if untouched {
        if text.trim() != seed.trim() {
            let _ = std::fs::write(&path, seed);
        }
        return seed.to_string();
    }
    text
}

/// Every section the system prompt can carry, in one place.
///
/// A struct, not nine positional arguments: the order used to be the only thing keeping
/// "memory" and "memory_index" apart, and the split below turns on knowing exactly which
/// sections are stable — a named field makes that a refactor, not a hunting trip through
/// call sites.
pub(crate) struct PromptParts<'a> {
    pub soul: &'a str,
    pub model: &'a str,
    pub provider: &'a str,
    pub memory: &'a str,
    pub memory_index: Option<&'a str>,
    /// Every skill's name plus its one-line description — small enough to ride along on
    /// every turn, so the model **chooses** a skill instead of hoping a keyword hits.
    pub skill_index: Option<&'a str>,
    /// The bodies of the skills whose trigger fired this turn (`$name` or a keyword match).
    pub skills: &'a str,
    pub todo: Option<&'a str>,
    pub summary: Option<&'a str>,
    /// The repository's own conventions (`AGENTS.md` upwards from the working directory).
    /// Sits in the dynamic half: it changes when the human switches project, not per turn,
    /// but it must not be part of the cached stable prefix either.
    pub instructions: Option<&'a str>,
    /// What this machine **cannot** do right now (no shell, no file writes). Said once per
    /// turn, in the prompt, because the alternative is the model discovering it by trial:
    /// a turn asked to clean up calendar events had no shell and no delete tool, so it spent
    /// eight iterations reading this repository's docs to find out where events live.
    pub capabilities: Option<&'a str>,
}

/// The system prompt cut in two at the cache boundary.
///
/// `stable` is byte-identical for the whole session (soul + model identity + conversation rules); `tail` is
/// everything that moves — the day, the todo list, memory hits, the skill catalog, the
/// rolling summary. Providers that want an explicit cache boundary (Anthropic) mark the
/// end of `stable`; the ones with automatic prefix caching simply need `stable` never to
/// move. Putting the clock in `tail` is the whole reason it is day-granular there: a
/// minute-level timestamp would invalidate the prefix sixty times an hour.
pub(crate) struct SystemPrompt {
    pub stable: String,
    pub tail: String,
}

impl SystemPrompt {
    /// One string, prefix first — for anyone who only wants to assert on the whole text
    /// (`full_turn` and the subagent pass the two halves through separately, so the cache
    /// boundary survives).
    #[cfg(test)]
    pub fn full(&self) -> String {
        if self.tail.is_empty() {
            self.stable.clone()
        } else {
            format!("{}\n{}", self.stable, self.tail)
        }
    }
}

/// What this machine cannot do right now, phrased as an instruction.
///
/// Two switches decide it, and both are off by default: `JEEVES_EXEC` (shell) and `JEEVES_WRITE`
/// (files). Without a line like this the model has no way to tell "the tool is missing" from
/// "I have not found the right tool yet" — it reads directories, greps docs and spends the
/// budget. Both Claude Code and Codex put their permission scope in the prompt for the same
/// reason; the difference is that theirs usually says *what is allowed*.
///
/// `None` when nothing is missing: the common case stays free.
pub(crate) fn capability_note(
    settings: &jeeves_config::Settings,
    tools_available: bool,
) -> Option<String> {
    if !tools_available {
        return Some("No tools are available for this turn. Tool references elsewhere do not grant access. Reply in plain text using the supplied context; explain when missing tools prevent verification. Do not emit tool-call syntax or claim a lookup or action occurred.".into());
    }
    let vault = vault_note(settings);
    let mut missing: Vec<&str> = Vec::new();
    if !settings.exec_enabled {
        missing.push("- running commands (shell) — off until the user turns it on in Settings (or sets `JEEVES_EXEC=1`)");
    }
    if !settings.write_enabled {
        missing.push("- writing or editing files — off until the user turns it on in Settings (or sets `JEEVES_WRITE=1`)");
    }
    if missing.is_empty() {
        return vault;
    }
    let mut note = format!(
        "What this machine cannot do right now:\n{}\nIf a request needs one of these, say so in one sentence and stop — do not approximate it with the tools you do have, and do not go reading source code to find a workaround.",
        missing.join("\n")
    );
    if !settings.exec_enabled {
        note.push_str(
            "\nSearching is not on that list: `glob` finds files by name and `grep` finds lines \
             by content, and neither needs a shell.",
        );
    }
    if let Some(vault) = vault {
        note.push_str("\n\n");
        note.push_str(&vault);
    }
    Some(note)
}

/// Which stored credentials a command may use. **Names only**: the values are injected into the one
/// process and scrubbed from its output, so the model can use a credential and never read it.
fn vault_note(settings: &jeeves_config::Settings) -> Option<String> {
    if !settings.exec_enabled {
        return None;
    }
    let names = jeeves_config::vault::names(&settings.home);
    if names.is_empty() {
        return None;
    }
    let vars: Vec<String> = names
        .iter()
        .map(|n| format!("${}{n}", jeeves_config::vault::ENV_PREFIX))
        .collect();
    Some(format!(
        "Stored credentials the owner lets commands use: {}. Reference them as environment variables inside run_command (e.g. `curl -H \"Authorization: Bearer $JEEVES_SECRET_NAME\" ...`). You cannot read their values and must never try to print them; the output is scrubbed.",
        vars.join(", ")
    ))
}

/// The minute a message was sent, put in front of the live ask. The system prompt carries only
/// the day so its cached prefix holds; "in two hours" or a `run_at` needs the minute, and it
/// rides on the one message that is new this turn. Stored history keeps the bare text.
pub(crate) fn sent_stamp(now: chrono::DateTime<Local>) -> String {
    format!(
        "[Sent {} ({}), UTC{}]",
        now.format("%Y-%m-%d %H:%M"),
        now.format("%A"),
        now.format("%:z")
    )
}

pub(crate) fn build_system(parts_in: PromptParts<'_>) -> SystemPrompt {
    let PromptParts {
        soul,
        model,
        provider,
        memory,
        memory_index,
        skill_index,
        skills,
        todo,
        summary,
        instructions,
        capabilities,
    } = parts_in;
    let now = Local::now();
    let stable = format!(
        "{soul}\nYour model: you are running on '{model}' via the '{provider}' provider, inside Jeeves, an open-source personal assistant.\n{CONVERSATION_RULES}"
    );

    let mut parts = vec![format!(
        "\nRight now it is {} ({}, UTC{})",
        now.format("%A, %Y-%m-%d"),
        now.format("%Z"),
        now.format("%:z")
    )];
    if let Some(capabilities) = capabilities {
        parts.push(format!("\n{capabilities}"));
    }
    parts.push("\nCapability use: discover available tools and the supplied Skill/MCP catalogs before concluding a capability is missing. In whatever execution mode the goal uses, compose existing tools first. When write/exec are enabled and the current policy allows it, implement a missing adapter as a small script inside the allowed working directory, validate it with representative inputs, then use it. Reuse verified local scripts on follow-up. Generated code inherits the same permissions, sandbox, budget and approval requirements; it cannot grant access, invent credentials, install global extensions or register model-controlled tasks. Report what was actually tested and what remains unavailable.".into());
    if let Some(todo) = todo {
        parts.push(format!("\n{todo}"));
    }
    if !memory.is_empty() {
        parts.push(format!("\nRelevant memory:\n{memory}"));
    }
    if let Some(index) = memory_index
        && !index.trim().is_empty()
    {
        parts.push(format!(
            "\nMemory library (catalog only — call read_page(kind=memory) for a page):\n{index}"
        ));
    }
    if let Some(index) = skill_index
        && !index.trim().is_empty()
    {
        parts.push(format!(
                "\nSkills (catalog only — call read_page(kind=skill) for a body when one fits):\n{index}"
            ));
    }
    if !skills.is_empty() {
        parts.push(format!("\nRelevant skill instructions:\n{skills}"));
    }
    if let Some(instructions) = instructions {
        parts.push(instructions.to_string());
    }
    if let Some(section) = summary.and_then(jeeves_memory::compaction::summary_section) {
        parts.push(section);
    }
    SystemPrompt {
        stable,
        tail: parts.join("\n"),
    }
}
