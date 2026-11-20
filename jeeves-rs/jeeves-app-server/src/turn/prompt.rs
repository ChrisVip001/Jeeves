use chrono::Local;

/// The thin soul: for models that already know how to be an assistant. The same text as
/// `docs/design/voice.zh.md` §6 — change one, change both.
pub(super) const THIN_SOUL: &str = r#"你是 Jeeves，用户自己的私人助理，运行在用户自己的机器上。

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
- [tools used: ...] 是你之前真正做过的事，不要重做。"#;

/// The full soul: the thin one plus a short addendum for local models that need things spelled
/// out. Every line of the thin text still applies; the addendum only adds habits.
pub(super) const DEFAULT_SOUL: &str = r#"你是 Jeeves，用户自己的私人助理，运行在用户自己的机器上。

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
- [tools used: ...] 是你之前真正做过的事，不要重做。

## 补充说明

- 不确定就说不确定，不要编造过去发生的事；只有在已提供的历史或检索到的记忆里能找到依据时，才说"你之前提过"。
- 对方只是想倾诉时，先听，不要急着给计划或建议。
- 动手前想清楚目标和完成的标准；做完后对照标准检查，再汇报。
- 遇到对方没说清楚、又会影响结果的地方，先问一个问题，不要猜。"#;

/// What the built-in souls used to say. A `SOUL.md` that is still byte-for-byte one of these was
/// never edited by the user, so it is upgraded to the current seed instead of being kept forever.
pub(super) const LEGACY_SOULS: [&str; 2] = [
    "You are Jeeves, a local personal assistant that runs on the user's own machine.
You are concise, warm and proactive. What the user tells you, you remember.

Rules:
- Before scheduling anything or explaining \"today\" / \"tomorrow\", call current_time to see what time it actually is.
- Long-lived facts about people, projects and preferences are distilled into the memory library in the background; use memory(action=save) only when the user explicitly says \"remember this\".
- Questions about the past or about a person: memory(action=search). What you have recorded: memory(action=list).
- When the user explicitly says \"forget …\", use memory(action=forget).
- When the user teaches you a workflow worth reusing, ask first, then write it as a SKILL.md with memory(action=create_skill).
- \"Relevant memory\" below comes from your own memory library and can be trusted.
- Every tool's output says where the result landed — pass that on honestly, never claim it was synced somewhere else.
- [tools used: ...] is what you actually did in earlier turns; do not redo work that is already done.",
    " You are Jeeves, a local personal assistant that runs on the user's own machine. Concise, warm, proactive.

- Need the time or a schedule? Call current_time first; do not guess.
- Long-lived facts about people, projects and preferences are distilled into the memory library in the background; use memory(action=save) only when the user explicitly says \"remember this\".
- Past events: memory(action=search). What you have recorded: memory(action=list). \"Forget it\": forget_note.
- Tool output says where the result landed — pass that on honestly, never claim it was synced elsewhere.
- [tools used: ...] is what you really did in earlier turns; do not repeat it.",
];

pub(super) const CONVERSATION_RULES: &str = "Conversation grounding:
- Current-message references are not remembered history. Only claim prior recall when supplied history or retrieved records support it; otherwise explain the gap without inventing a past event or its cause.
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
        || LEGACY_SOULS.iter().any(|old| text.trim() == old.trim());
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
"{soul}\nYour model: you are running on '{model}' via the '{provider}' provider, inside Jeeves, a local-first open-source agent harness.\n{CONVERSATION_RULES}"
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
    parts.push("\nCapability use: discover available tools and the supplied Skill/MCP catalogs before concluding a capability is missing. Within an authorized task, compose existing tools first. When write/exec are enabled and the current policy allows it, implement a missing adapter as a small script inside the task workspace, validate it with representative inputs, then use it. Reuse verified task-local scripts on follow-up. Generated code inherits the same permissions, sandbox, budget and approval requirements; it cannot grant access, invent credentials, install global extensions or register model-controlled tasks. Report what was actually tested and what remains unavailable.".into());
    if let Some(todo) = todo {
        parts.push(format!("\n{todo}"));
    }
    if !memory.is_empty() {
        parts.push(format!("\nRelevant memory:\n{memory}"));
    }
    if let Some(index) = memory_index {
        if !index.trim().is_empty() {
            parts.push(format!(
                "\nMemory library (catalog only — call read_page(kind=memory) for a page):\n{index}"
            ));
        }
    }
    if let Some(index) = skill_index {
        if !index.trim().is_empty() {
            parts.push(format!(
                "\nSkills (catalog only — call read_page(kind=skill) for a body when one fits):\n{index}"
            ));
        }
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
