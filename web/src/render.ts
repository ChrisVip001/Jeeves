
import { renderMarkdown, stripToolFold } from "./markdown.ts";
import { approvalOutcome } from "./task-event-text.ts";
import { t } from "./i18n.ts";
import { readableMinute } from "./when.ts";
import type {
  CapabilityView,
  McpRow,
  McpView,
  ScheduleJob,
  SchedulesView,
  SkillRow,
  SkillsView,
} from "./api";
import type {
  AgentTaskView,
  ArtifactView,
  AcceptanceView,
  BudgetView,
  CheckView,
  ContextUsage,
  DashboardData,
  LedgerEntryView,
  LedgerTurnView,
  MemoryPageSummary,
  MemoryPageView,
  Message,
  ReceiptApproval,
  TodoListView,
  TokenUsage,
  ToolStartedNotification,
  TurnCompletedNotification,
  TurnMeta,
} from "./protocol";

type Attrs = Record<
  string,
  string | number | boolean | undefined | (() => void)
>;

/**
 * Build an element.
 *
 * Text always goes through `textContent`: what the model says, memory contents, file paths are
 * all external input, with no reason to ever become HTML. `innerHTML` has no use in this page.
 */
export function el<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  attrs: Attrs = {},
  ...children: (Node | string)[]
): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(attrs)) {
    if (value === undefined || value === false) continue;
    if (key === "class") node.className = String(value);
    else if (key === "text") node.textContent = String(value);
    else if (key.startsWith("on") && typeof value === "function") {
      node.addEventListener(key.slice(2), value as EventListener);
    } else node.setAttribute(key, String(value));
  }
  for (const child of children) node.append(child);
  return node;
}

/** Overview panel for the current Runtime. */
export function renderOverview(
  data: DashboardData,
  onTask: (
    task: AgentTaskView,
    action:
      | "start"
      | "wait"
      | "input"
      | "operations"
      | "suspend"
      | "resume"
      | "cancel",
  ) => void,
): DocumentFragment {
  const { config } = data;
  const fragment = document.createDocumentFragment();

  if (data.todos !== undefined || data.task != null) {
    const emptyTodos: TodoListView = { revision: 0n, items: [] };
    fragment.append(
      renderTaskCard(
        data.todos ?? emptyTodos,
        data.task,
        data.taskArtifacts ?? [],
        onTask,
      ),
    );
  }

  fragment.append(
    el(
      "div",
      { class: "headline" },
      el("div", { class: "provider", text: config.provider }),
      el("div", { class: "model", text: config.model }),
    ),
    el("div", { class: "path", text: config.home, title: config.home }),
    numbers([
      [t("事实"), data.facts.length],
      [t("情景"), data.episodes.length],
    ]),
  );

  fragment.append(
    section(
      t("怎么想的"),
      el(
        "dl",
        { class: "rows" },
        ...row(t("小模型"), config.smallModel),
        ...row(
          t("工作记忆"),
          t("最近 {historyTurns} 轮", { historyTurns: config.historyTurns }),
        ),
        ...row(
          t("提炼频率"),
          t("每 {consolidateEvery} 轮", {
            consolidateEvery: config.consolidateEvery,
          }),
        ),
        ...row(t("检索条数"), String(config.retrievalTopK)),
      ),
    ),
  );

  if (data.connectors.length > 0 || data.connectorsError != null) {
    fragment.append(renderConnectors(data));
  }

  return fragment;
}



function renderConnectors(data: DashboardData): HTMLElement {
  const names: Record<string, string> = { feishu: t("飞书") };
  const block = el(
    "div",
    { class: "block connectors" },
    el("h3", { text: t("连接器") }),
  );
  if (data.connectorsError !== null) {
    block.append(
      el("div", { class: "connector-broken", text: data.connectorsError }),
    );
  }
  for (const connector of data.connectors) {
    block.append(
      el(
        "div",
        { class: "connector-row" },
        el("span", {
          class: "connector-name",
          text: names[connector.id] ?? connector.id,
        }),
        el("span", {
          class: `tag connector-mode ${connector.live ? "live" : "draft"}`,
          text: connector.live ? t("可发送") : t("草稿"),
        }),
        el("span", {
          class: "connector-allow",
          text:
            connector.allowRecipients === 0
              ? t("白名单空 —— 谁都不能收")
              : t("白名单 {allowRecipients} 个", {
                  allowRecipients: connector.allowRecipients,
                }),
        }),
      ),
    );
  }
  block.append(
    el("p", {
      class: "connector-note",
      text: t("开关在 connectors.json 里 —— 只有人能改；实际发送每次都会问。"),
    }),
  );
  return block;
}

/**
 * The task card: the task (which is also where a goal lives) plus the model's own todo sheet,
 * as the human sees them. Both come from `DashboardData` (task/todos, filled only when the page
 * names a session) — this is where "what is the agent actually working on" lives.
 */
export function renderTaskCard(
  todos: TodoListView,
  task: AgentTaskView | undefined,
  artifacts: ArtifactView[],
  onTask: (
    task: AgentTaskView,
    action:
      | "start"
      | "wait"
      | "input"
      | "operations"
      | "suspend"
      | "resume"
      | "cancel",
  ) => void,
): DocumentFragment {
  const fragment = document.createDocumentFragment();
  const block = el("div", { class: "block taskcard" });

  const statusText: Record<string, string> = {
    pending: t("等待开始"),
    running: t("进行中"),
    suspended: t("已暂停"),
    needs_attention: t("需要你确认"),
    done: t("已完成"),
    failed: t("未达成"),
    cancelled: t("已取消"),
  };
  if (task != null) {
    const live = task.status === "running" || task.status === "pending";
    const automaticWait =
      task.status === "suspended" &&
      [
        "waiting for background jobs",
        "waiting for authorized external input",
      ].includes(task.suspendReason ?? "");
    const resumable =
      task.status === "needs_attention" ||
      (task.status === "suspended" && !automaticWait);
    const open = live || resumable || automaticWait;
    block.append(
      el("h3", { text: t("任务") }),
      el(
        "div",
        { class: "task-goal" },
        el("span", { class: "task-goal-text", text: task.title }),
        el("span", {
          class: `tag task-state st-${task.status}`,
          text:
            (task.status === "done" && task.acceptance.length === 0
              ? t("已产出，未核验")
              : task.status === "suspended" &&
                  task.suspendReason === "waiting for authorized external input"
                ? t("等待资料")
                : task.status === "suspended" &&
                    task.suspendReason === "waiting for background jobs"
                  ? t("等待后台作业")
                  : task.status === "pending" &&
                      task.dispatchState === "pending"
                    ? t("排队中")
                    : (statusText[task.status] ?? task.status)) +
            (task.goalCondition != null
              ? t(" · 第 {round}/{max} 轮", {
                  round: task.roundsUsed,
                  max: task.maxRounds,
                })
              : ""),
        }),
      ),
      task.suspendReason != null
        ? el("div", { class: "task-note", text: task.suspendReason })
        : document.createDocumentFragment(),
      ...(task.inputCondition
        ? [
            el(
              "div",
              { class: "task-note" },
              el("p", { text: task.inputCondition.description }),
              el("p", {
                text: task.inputCondition.eventId
                  ? t("资料已收到")
                  : t("资料尚未到达"),
              }),
              el("p", {
                text:
                  task.inputCondition.source === "manual"
                    ? t("由你提供")
                    : t("来源：{source}", {
                        source: task.inputCondition.source,
                      }),
              }),
              ...(task.inputCondition.expiresAt
                ? [
                    el("p", {
                      text: t("最晚到达：{time}", {
                        time: new Date(
                          task.inputCondition.expiresAt,
                        ).toLocaleString(),
                      }),
                    }),
                  ]
                : []),
              ...(task.inputCondition.data !== undefined
                ? [
                    el(
                      "details",
                      {},
                      el("summary", { text: t("查看资料") }),
                      el("pre", {
                        class: "task-input-data",
                        text:
                          typeof task.inputCondition.data === "string"
                            ? task.inputCondition.data
                            : JSON.stringify(task.inputCondition.data, null, 2),
                      }),
                    ),
                  ]
                : []),
            ),
          ]
        : []),
      ...acceptanceBlock(task.acceptance, task.checks),
      ...artifactBlock(artifacts),
      ...budgetBlock(task.budget, task.spent),
      taskFooter(task),
      el(
        "div",
        { class: "task-actions" },
        ...(task.status === "pending" && task.dispatchState === undefined
          ? [
              el("button", {
                class: "task-act task-act-go",
                type: "button",
                text: t("开始任务"),
                onclick: () => onTask(task, "start"),
              }),
            ]
          : []),
        ...(task.status === "pending" &&
        task.dispatchState === undefined &&
        task.runCount === 0 &&
        !task.inputCondition
          ? [
              el("button", {
                class: "task-act",
                type: "button",
                text: t("等待资料"),
                onclick: () => onTask(task, "wait"),
              }),
            ]
          : []),
        ...(open &&
        task.inputCondition?.source === "manual" &&
        !task.inputCondition.eventId
          ? [
              el("button", {
                class: "task-act",
                type: "button",
                text: t("提供资料"),
                onclick: () => onTask(task, "input"),
              }),
            ]
          : []),
        ...(live || automaticWait
          ? [
              el("button", {
                class: "task-act",
                type: "button",
                text: t("暂停"),
                onclick: () => onTask(task, "suspend"),
              }),
            ]
          : []),
        ...(resumable && task.runCount > 0
          ? [
              el("button", {
                class: "task-act",
                type: "button",
                text: t("核对操作"),
                onclick: () => onTask(task, "operations"),
              }),
            ]
          : []),
        ...(resumable
          ? [
              el("button", {
                class: "task-act task-act-go",
                type: "button",
                text: t("继续"),
                onclick: () => onTask(task, "resume"),
              }),
            ]
          : []),
        ...(open
          ? [
              el("button", {
                class: "task-act task-act-stop",
                type: "button",
                text: t("取消"),
                onclick: () => onTask(task, "cancel"),
              }),
            ]
          : []),
      ),
    );
  }

  if (todos.items.length > 0) {
    block.append(
      el("h3", { text: t("待办") }),
      el(
        "ul",
        { class: "task-todos" },
        ...todos.items.map((item) =>
          el(
            "li",
            { class: `task-todo st-${item.status}` },
            el("span", { class: "task-glyph", "aria-hidden": "true" }),
            el("span", { class: "task-text", text: item.content }),
          ),
        ),
      ),
    );
  }

  if (block.children.length > 0) fragment.append(block);
  return fragment;
}

/** At most this many condition rows before folding into "{N} more". */
const CHECKS_SHOWN = 6;
/** At most this many deliverables before folding. */
const ARTIFACTS_SHOWN = 5;

/**
 * The acceptance checklist: **the reason** a task is not done, in the same words the checker
 * used. `checks` is index-aligned with `acceptance`; a condition with no verdict yet says so
 * instead of pretending to pass.
 */
function acceptanceBlock(
  acceptance: AcceptanceView[],
  checks: CheckView[],
): Node[] {
  if (acceptance.length === 0) return [];
  const rows = acceptance.slice(0, CHECKS_SHOWN).map((condition, index) => {
    const check = checks[index];
    const passed = check?.passed === true;
    return el(
      "li",
      { class: `task-check ${passed ? "ok" : "bad"}` },
      el("span", { class: "task-glyph", "aria-hidden": "true" }),
      el("span", {
        class: "task-check-label",
        text: condition.label ?? check?.label ?? condition.kind,
      }),
      el("span", {
        class: "task-check-detail",
        title: check?.artifactSha256
          ? t("核验时的文件版本：{hash}", { hash: check.artifactSha256 })
          : "",
        text: check?.detail ?? t("尚未检查"),
      }),
    );
  });
  const rest = acceptance.length - rows.length;
  return [
    el("h3", { text: t("验收") }),
    el("ul", { class: "task-checks" }, ...rows),
    rest > 0
      ? el("div", {
          class: "task-more",
          text: t("还有 {rest} 条", { rest: rest }),
        })
      : document.createDocumentFragment(),
  ];
}

/** The deliverables: files that exist, with their real sizes. */
function artifactBlock(artifacts: ArtifactView[]): Node[] {
  if (artifacts.length === 0) return [];
  const rows = artifacts
    .slice(0, ARTIFACTS_SHOWN)
    .map((artifact) =>
      el(
        "li",
        { class: "task-artifact", title: artifact.path },
        el("span", {
          class: "task-artifact-name",
          text: baseName(artifact.path),
        }),
        el("span", {
          class: "task-artifact-size",
          text: formatBytes(artifact.bytes),
        }),
      ),
    );
  const rest = artifacts.length - rows.length;
  return [
    el("h3", { text: t("交付物") }),
    el("ul", { class: "task-artifacts" }, ...rows),
    rest > 0
      ? el("div", {
          class: "task-more",
          text: t("还有 {rest} 个", { rest: rest }),
        })
      : document.createDocumentFragment(),
  ];
}

/**
 * Three usage bars (time / tokens / tool calls). A dimension with no cap still shows what was
 * spent — the numbers are the honest part; the bar only exists where there is a limit to hit.
 */
function budgetBlock(
  budget: BudgetView | null,
  spent: { wallMs: number; tokens: number; tools: number },
): Node[] {
  if (budget == null) return [];
  const clock = (budget.wallMs ?? spent.wallMs) >= 60_000;
  const time = (value: number) => formatDuration(value, clock);
  const rows = [
    budgetRow(t("时长"), time(spent.wallMs), spent.wallMs, budget.wallMs, time),
    budgetRow(
      "Token",
      String(spent.tokens),
      spent.tokens,
      budget.tokens,
      String,
    ),
    budgetRow(
      t("工具调用"),
      String(spent.tools),
      spent.tools,
      budget.tools,
      String,
    ),
  ].filter((row): row is HTMLDivElement => row !== null);
  if (rows.length === 0) return [];
  return [
    el("h3", { text: t("预算") }),
    el("div", { class: "task-budget" }, ...rows),
  ];
}

function budgetRow(
  name: string,
  shown: string,
  spent: number,
  cap: number | null,
  render: (value: number) => string,
): HTMLDivElement | null {
  if (cap === null && spent === 0) return null;
  const ratio = cap !== null && cap > 0 ? spent / cap : null;
  const level =
    ratio === null
      ? ""
      : ratio >= 1
        ? " is-over"
        : ratio >= 0.8
          ? " is-warn"
          : "";
  return el(
    "div",
    { class: "task-budget-row" },
    el(
      "div",
      { class: "task-budget-line" },
      el("span", { class: "task-budget-name", text: name }),
      el("span", {
        class: "task-budget-num",
        text: cap === null ? shown : `${shown} / ${render(cap)}`,
      }),
    ),
    ratio === null
      ? document.createDocumentFragment()
      : el(
          "div",
          {
            class: `meter${level}`,
            role: "img",
            "aria-label": `${name} ${shown} / ${render(cap as number)}`,
          },
          el("div", {
            class: "meter-fill",
            style: `width:${Math.min(100, Math.round(ratio * 100))}%`,
          }),
        ),
  );
}

/** "3 runs · last stopped at "acceptance failed: report.md does not exist"" with no budget set */
function taskFooter(task: AgentTaskView): Node {
  const parts: string[] = [];
  parts.push(
    task.runCount > 0
      ? t("执行 {runCount} 次", { runCount: task.runCount })
      : t("还没执行过"),
  );
  if (task.lastStopReason !== null && task.lastStopReason !== "") {
    parts.push(
      t("上次停在「{lastStopReason}」", {
        lastStopReason: task.lastStopReason,
      }),
    );
  }
  if (task.budget == null) parts.push(t("未设预算"));
  const text = parts.join(" · ");
  return el("div", { class: "task-foot", text, title: text });
}

function baseName(path: string): string {
  const cut = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  return cut >= 0 ? path.slice(cut + 1) : path;
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

/** `clock` = the row is long enough that minutes matter ("0:45 / 10:00" beats "45.2s / 600s"). */
function formatDuration(ms: number, clock: boolean): string {
  if (!clock) return `${(ms / 1000).toFixed(1)}s`;
  const minutes = Math.floor(ms / 60_000);
  const seconds = Math.round((ms % 60_000) / 1000);
  return `${minutes}:${String(seconds).padStart(2, "0")}`;
}

/** What a notification said (gate / tool), as a small capsule. */
export function chip(
  text: string,
  kind: "gate" | "tool" | "note" = "note",
): HTMLElement {
  return el("span", { class: `chip ${kind}`, text });
}

/** The window event that asks the cockpit to reopen first-run model setup. */
export const SETUP_EVENT = "jeeves:setup";

/** Runtime messages a person can act on, shown as a plain sentence and a next step. */
function known(message: string): { title: string; body: string } | null {
  if (message.startsWith("Configure a model before sending a message"))
    return {
      title: t("还没连接模型"),
      body: t("Jeeves 要先连上一个模型才能回复。连接好后，重新发送这条消息即可。"),
    };
  return null;
}

/**
 * Error: a quiet red card, not an alarm. The protocol code stays as a tag —
 * useful when checking logs, meaningless to a person. A known cause gets a
 * plain sentence and a button that fixes it instead of the raw message.
 */
export function failure(error: { code: number; message: string }): HTMLElement {
  const plain = known(error.message);
  const card = el(
    "div",
    { class: plain ? "failure actionable" : "failure" },
    el(
      "div",
      { class: "failure-head" },
      el("span", { class: "failure-dot", "aria-hidden": "true" }),
      el("span", { class: "failure-title", text: plain?.title ?? t("出错了") }),
      el("span", { class: "code", text: String(error.code) }),
    ),
    el("div", { class: "failure-body", text: plain?.body ?? error.message }),
  );
  if (plain) {
    const fix = el("button", {
      type: "button",
      class: "failure-action",
      text: t("连接模型"),
    });
    fix.addEventListener("click", () =>
      window.dispatchEvent(new CustomEvent(SETUP_EVENT)),
    );
    card.append(fix);
  }
  return card;
}

export function failureOf(error: unknown): HTMLElement {
  if (error instanceof Error) {
    const code = (error as { code?: unknown }).code;
    return failure({
      code: typeof code === "number" ? code : 0,
      message: error.message,
    });
  }
  return failure({ code: 0, message: String(error) });
}

/** The small line when a turn ends: which model, how long, how many tokens. */
export function turnFooter(turn: TurnCompletedNotification): string {
  const parts = [
    turn.meta.model,
    t("{iterations} 轮", { iterations: turn.meta.iterations }),
    formatMs(turn.meta.latencyMs),
  ];
  if (turn.usage !== null && turn.usage !== undefined)
    parts.push(formatUsage(turn.usage));
  if (turn.meta.tools.length > 0)
    parts.push(t("{length} 次工具", { length: turn.meta.tools.length }));
  if (turn.meta.interrupted === true) parts.push(t("已被你停止"));
  if (turn.meta.routeReason)
    parts.push(t("路由 {routeReason}", { routeReason: turn.meta.routeReason }));
  return parts.join(" · ");
}

/**
 * The empty-session welcome: a quiet hero instead of an orphan sentence.
 * Rendered only when a session has no history; the first message replaces it.
 */
/** Starter requests on the empty chat; each one shows what Jeeves can take on. */
/** One empty-chat suggestion: a category eyebrow, the prompt it fills, and an icon glyph. */
interface Starter {
  category: string;
  prompt: string;
  /** A single SVG path (24×24 viewBox, Lucide-style) drawn in the card corner. */
  icon: string;
}

function starters(): Starter[] {
  return [
    {
      category: t("提醒"),
      prompt: t("明天上午 9 点提醒我给房东打电话"),
      icon: "M12 6v6l4 2 M12 22a10 10 0 1 0 0-20 10 10 0 0 0 0 20z",
    },
    {
      category: t("每日简报"),
      prompt: t("每个工作日早上 8 点给我一份今日简报"),
      icon: "M4 4h16v16H4z M8 9h8 M8 13h8 M8 17h5",
    },
    {
      category: t("调研"),
      prompt: t("帮我调研一下适合家用的空气净化器，列出优缺点"),
      icon: "M11 18a7 7 0 1 0 0-14 7 7 0 0 0 0 14z M21 21l-4.3-4.3",
    },
    {
      category: t("整理"),
      prompt: t("整理我的下载文件夹，先告诉我打算怎么分类"),
      icon: "M3 7h6l2 2h10v10a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1z",
    },
  ];
}

/** A 24×24 SVG built from one or more `M…` path strings. */
function starterIcon(paths: string): SVGElement {
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("fill", "none");
  svg.setAttribute("stroke", "currentColor");
  svg.setAttribute("stroke-width", "1.7");
  svg.setAttribute("stroke-linecap", "round");
  svg.setAttribute("stroke-linejoin", "round");
  svg.setAttribute("aria-hidden", "true");
  for (const d of paths.split(" M").map((part, i) => (i === 0 ? part : "M" + part))) {
    const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
    path.setAttribute("d", d.trim());
    svg.append(path);
  }
  return svg;
}

/**
 * The empty-chat screen. Picking a suggestion fills the composer for editing; it never sends.
 */
export function welcome(pick: (text: string) => void): HTMLElement {
  const list = el("div", { class: "welcome-starters", role: "list" });
  for (const { category, prompt, icon } of starters()) {
    const card = el(
      "button",
      { type: "button", class: "welcome-starter", role: "listitem" },
      el("span", { class: "welcome-starter-icon" }, starterIcon(icon)),
      el("span", { class: "welcome-starter-eyebrow", text: category }),
      el("span", { class: "welcome-starter-text", text: prompt }),
    );
    card.addEventListener("click", () => pick(prompt));
    list.append(card);
  }
  return el(
    "div",
    { class: "welcome" },
    el("img", {
      class: "welcome-logo",
      src: "./icon.svg",
      alt: "",
      "aria-hidden": "true",
    }),
    el("h1", { class: "welcome-title", text: t("交给 Jeeves 什么事？") }),
    el("p", { class: "welcome-sub", text: t("挑一个开始，或者直接说你想做什么") }),
    list,
  );
}

/**
 * The attachment strip under a sent message: thumbnails for images, name chips for the rest.
 * `url` is an object URL minted by main (it owns the lifecycle); without one, name only.
 */
export function attachmentStrip(
  files: { name: string; url?: string }[],
): HTMLElement {
  const strip = el("div", { class: "attach-strip" });
  for (const file of files) {
    if (file.url !== undefined) {
      const img = el("img", {
        class: "attach-thumb",
        src: file.url,
        alt: file.name,
        title: file.name,
      });
      img.onerror = () => {
        img.replaceWith(el("span", { class: "tag", text: file.name }));
      };
      strip.append(img);
    } else {
      strip.append(
        el("span", { class: "tag", text: file.name, title: file.name }),
      );
    }
  }
  return strip;
}

/**
 * One message from history.
 *
 * Deliberately painted identical to the **live stream** (same classes, same layout): after a
 * refresh a message shouldn't look like a different client — they're two sources of one talk.
 */
export function taskNotice(message: Message): "task_decision" | "task_completion" | undefined {
  const source = message.source;
  return source === "task_decision" || source === "task_completion" ? source : undefined;
}

export function taskNoticeHeading(message: Message): string {
  if (message.source === "task_decision") return t("审批记录");
  switch (message.receiptStatus) {
    case "done": return t("验收通过");
    case "produced_unverified": return t("已生成，待验收");
    case "failed": return t("执行失败");
    case "cancelled": return t("已取消");
    default: return t("任务回执");
  }
}

/** Render the run's decisions independently of the worker's account of what happened. */
export function taskReceiptBody(decisions: ReceiptApproval[] | undefined, reply: string): DocumentFragment {
  const body = document.createDocumentFragment();
  if (decisions?.length) {
    body.append(el("strong", { text: t("本次审批结果") }));
    const list = el("ul", { class: "receipt-decisions" });
    for (const row of decisions) {
      list.append(el("li", {}, el("strong", { text: approvalOutcome(row.outcome, t) }),
        " · ", el("code", { text: row.tool }), " · ", el("small", { text: row.requestId })));
    }
    body.append(list);
  } else {
    body.append(el("p", { text: decisions ? t("本次执行没有已记录的审批决定。") : t("无法关联本次执行的审批记录。") }));
  }
  if (reply) {
    const text = el("div", { class: "bubble md" });
    text.append(renderMarkdown(stripToolFold(reply)));
    const original = el("details", {}, el("summary", { text: t("查看任务原回复") }), text);
    original.open = decisions?.length === 0;
    body.append(original);
  }
  return body;
}

export function messageRow(message: Message, openTask: (id: string) => Promise<boolean>): HTMLElement {
  const notice = taskNotice(message);
  if (notice) {
    const decision = notice === "task_decision";
    const split = message.content.indexOf("\n\n");
    const summary = split >= 0 ? message.content.slice(split + 2) : "";
    const heading = taskNoticeHeading(message);
    const firstLine = message.content.split("\n", 1)[0] ?? "";
    const suffix = " · " + message.taskId;
    const title = message.taskId && firstLine.endsWith(suffix) ? firstLine.slice(0, -suffix.length) : "";
    const row = el("div", { class: `msg jeeves task-notice ${notice}` },
      el("strong", { text: title ? title + " · " + heading : heading }));
    if (!decision) {
      row.append(taskReceiptBody(message.approvalDecisions, summary));
    }
    if (decision || !summary) {
      row.append(el("details", {}, el("summary", { text: decision ? t("查看原始请求") : t("执行记录") }), el("div", { class: "bubble", text: message.content })));
    }
    const taskId = message.taskId;
    if (taskId) {
      const status = el("span", { role: "status" });
      const label = ["done", "produced_unverified"].includes(message.receiptStatus ?? "") ? t("继续这件事") : t("查看任务对话");
      const button = el("button", { type: "button", class: "today-btn", text: label });
      button.addEventListener("click", async () => {
        status.textContent = await openTask(taskId) ? "" : t("暂时无法打开原任务，请重试。");
      });
      row.append(button);
      row.append(status);
    }
    row.dataset.noticeId = String(message.id);
    row.setAttribute("role", "status");
    return row;
  }
  if (message.role === "assistant" && message.source?.startsWith("responsibility:")) {
    const row = el("div", { class: "msg jeeves", role: "status" },
      el("div", { class: "bubble", text: message.content }));
    row.dataset.noticeId = String(message.id);
    return row;
  }
  if (message.role === "user") {
    return el(
      "div",
      { class: "msg me" },
      el("div", { class: "bubble", text: message.content }),
    );
  }

  const bubble = el(
    "div",
    { class: "bubble md" },
    renderMarkdown(stripToolFold(message.content)),
  );

  const meta = message.meta;
  const trace = el(
    "div",
    { class: "trace" },
    ...(meta === null || meta === undefined
      ? []
      : [metaTrace(meta), ...metaChips(meta)]),
  );

  return el(
    "div",
    { class: "msg jeeves" },
    bubble,
    el("details", { class: "message-meta" },
      el("summary", { text: formatWhen(message.at), title: t("执行记录") }),
      trace, el("div", { class: "foot", text: historyFooter(message) })),
  );
}

/** The persisted tool calls, folded inside the timestamp details. */
function metaTrace(meta: TurnMeta): HTMLElement {
  const latest = meta.tools.at(-1);
  return el(
    "details",
    { class: "trace-detail" },
    el("summary", {
      text:
        meta.tools.length === 0
          ? t("没有工具调用")
          : t("{n} 步执行 · {tool}", {
              n: meta.tools.length,
              tool: latest?.tool ?? "",
            }),
    }),
    ...meta.tools.map((tool) =>
      el(
        "div",
        {
          class: `trace-row ${tool.status === "ok" ? "done-ok" : "done-bad"}`,
        },
        el("span", { class: "trace-name", text: tool.tool }),
        el("span", {
          class: "trace-status",
          text: tool.status === "ok" ? t("成功") : t("失败"),
        }),
        el("span", {
          class: "trace-ms",
          text: tool.durationMs === null || tool.durationMs === undefined ? "" : formatMs(tool.durationMs),
        }),
      ),
    ),
  );
}

/** Persisted telemetry → capsules identical to the live view. */
function metaChips(meta: TurnMeta): HTMLElement[] {
  const chips: HTMLElement[] = [];

  for (const tool of meta.tools) {
    chips.push(
      chip(
        t("工具 · {tool} {status}", {
          tool: tool.tool,
          status: tool.status === "ok" ? t("好了") : t("出错"),
        }),
        "tool",
      ),
    );
  }

  return chips;
}

/**
 * The small line in history.
 *
 * **No token counts**: usage only passed through `turnCompleted` and never entered the store —
 * rather than guess a number, write only what the store truly has (model, iterations, timing).
 */
function historyFooter(message: Message): string {
  const parts = [formatWhen(message.at)];
  const meta = message.meta;
  if (meta !== null && meta !== undefined) {
    parts.unshift(
      meta.model,
      t("{iterations} 轮", { iterations: meta.iterations }),
      formatMs(meta.latencyMs),
    );
  }
  return parts.join(" · ");
}


function section(title: string, body: Node): HTMLElement {
  return el("div", { class: "block" }, el("h3", { text: title }), body);
}

/** The "there's nothing here" line. */
export function muted(text: string): HTMLElement {
  return el("p", { class: "muted", text });
}

function numbers(pairs: [string, number][]): HTMLElement {
  return el(
    "div",
    { class: "numbers" },
    ...pairs.map(([label, value]) =>
      el(
        "div",
        { class: "number" },
        el("b", { text: String(value) }),
        el("span", { text: label }),
      ),
    ),
  );
}

function row(label: string, value: string): [HTMLElement, HTMLElement] {
  return [el("dt", { text: label }), el("dd", { text: value, title: value })];
}

export function formatWhen(iso: string | null | undefined): string {
  if (iso === null || iso === undefined || iso === "") return "—";
  const at = new Date(iso);
  if (Number.isNaN(at.getTime())) return iso;

  const minutes = (Date.now() - at.getTime()) / 60_000;
  if (minutes < 1) return t("刚刚");
  if (minutes < 60) return t("{n} 分钟前", { n: Math.floor(minutes) });
  if (minutes < 60 * 24) return clock(at);
  return t("{month} 月 {day} 日", {
    month: at.getMonth() + 1,
    day: at.getDate(),
  });
}

/** The header timestamp: when the data is from. */
export function formatStamp(iso: string): string {
  const at = new Date(iso);
  return Number.isNaN(at.getTime())
    ? iso
    : t("{time} 更新", { time: clock(at) });
}

export function formatMs(ms: number): string {
  return ms < 1000 ? `${ms}ms` : `${(ms / 1000).toFixed(1)}s`;
}

function formatUsage(usage: TokenUsage): string {
  return `${usage.inputTokens}→${usage.outputTokens} token`;
}

function clock(at: Date): string {
  return at.toLocaleTimeString("zh-CN", {
    hour: "2-digit",
    minute: "2-digit",
    hour12: false,
  });
}


type ToolArgs = Record<string, unknown>;

function argString(args: ToolArgs, ...keys: string[]): string {
  for (const key of keys) {
    const value = args[key];
    if (typeof value === "string" && value !== "") return value;
  }
  return "";
}

/** Tool name → a one-line summary of "what this step is doing". */
const TOOL_SUMMARY: Record<string, (args: ToolArgs) => string> = {
  run_command: (args) => argString(args, "command"),
  read_file: (args) => argString(args, "path"),
  write_file: (args) => argString(args, "path"),
  edit: (args) => argString(args, "path"),
  glob: (args) => argString(args, "pattern", "path") || ".",
  grep: (args) => argString(args, "pattern"),
  fetch_url: (args) => argString(args, "url"),
  search_web: (args) => argString(args, "query"),
  delegate_task: (args) => argString(args, "task", "description"),
  memory: (args) => argString(args, "content", "query"),
  read_page: (args) => argString(args, "name"),
  todo_write: (args) => {
    const todos = args["todos"];
    return Array.isArray(todos)
      ? t("{length} 项", { length: todos.length })
      : t("看一眼");
  },
};

/** A one-line plain-language summary of "what this step is doing" (empty = show name only). */
export function toolSummary(notification: ToolStartedNotification): string {
  const args =
    typeof notification.args === "object" && notification.args !== null
      ? (notification.args as ToolArgs)
      : {};
  const summarize = TOOL_SUMMARY[notification.tool];
  return summarize ? summarize(args) : "";
}

/** The stopwatch text in a card. `start` is the moment this card was created. */
export function elapsedSince(start: number): string {
  return formatMs(Date.now() - start);
}


/** The read-only inventory: what this machine has, and which human switches are on. */
export function renderCapabilities(view: CapabilityView): DocumentFragment {
  const fragment = document.createDocumentFragment();
  fragment.append(
    el("p", {
      class: "hint",
      text: t("这台机器上的能力总览；这里只查看，不在这里修改。"),
    }),
    el("p", {
      class: "cap-note",
      text: t("技能 {n} 个 · 连接器 {m} 个 · 开关 {k} 个", {
        n: view.skills.length,
        m: view.connectors.length,
        k: view.switches.length,
      }),
    }),
  );
  if (view.error !== null) {
    fragment.append(el("div", { class: "failure", text: view.error }));
  }
  const labels: Record<string, string> = {
    exec: t("命令执行"),
    write: t("文件写入"),
    delegate: t("子代理"),
  };
  for (const row of view.switches) {
    fragment.append(
      el(
        "div",
        { class: "cap-row" },
        el(
          "div",
          { class: "cap-row-head" },
          el("span", { class: "cap-name", text: labels[row.id] ?? row.id }),
          el("span", {
            class: "tag",
            text: row.enabled ? t("已开启") : t("已关闭"),
          }),
        ),
        capMeta([
          row.source === "settings" ? t("取自设置") : t("取自环境变量"),
        ]),
      ),
    );
  }
  fragment.append(
    el("p", {
      class: "cap-note",
      text: t("技能与连接器在各自页签里管理；能力开关由 CLI 与配置设置。"),
    }),
  );
  return fragment;
}

/** The skill list: what Jeeves knows how to do, and which rows belong to the human. */
export function renderSkills(
  view: SkillsView,
  onAdd: () => void,
  onRemove: (skill: SkillRow) => void,
): DocumentFragment {
  const fragment = document.createDocumentFragment();
  fragment.append(
    capHead(
      t("技能 · {length} 个", { length: view.skills.length }),
      t("＋ 添加技能"),
      onAdd,
    ),
  );

  if (view.error !== null) {
    fragment.append(el("div", { class: "failure", text: view.error }));
  }
  if (view.skills.length === 0) {
    fragment.append(
      muted(
        t(
          "还没有技能。技能是一份 SKILL.md：写上名字、用途和做法，Jeeves 会在相关消息出现时载入它。",
        ),
      ),
    );
  }

  const loaded = view.skills.map((skill) => skill.name);
  for (const skill of view.skills) {
    const missing = skill.dependencies.filter((dep) => !loaded.includes(dep));
    const row = el(
      "div",
      {
        class: `cap-row${skill.source === "builtin" ? " builtin" : ""}`,
        title:
          skill.source === "builtin"
            ? t(
                "「{name}」随 Jeeves 内置分发（JEEVES_SKILL_DIRS）—— 只能看，不能改",
                { name: skill.name },
              )
            : `「${skill.name}」 · ${skill.folder}`,
      },
      el(
        "div",
        { class: "cap-row-head" },
        el("span", { class: "cap-name", text: skill.name }),
        skill.version !== null
          ? el("span", { class: "tag", text: `v${skill.version}` })
          : document.createDocumentFragment(),
        skill.source === "builtin"
          ? el("span", { class: "tag cap-builtin-tag", text: t("内置") })
          : document.createDocumentFragment(),
        skill.source === "user"
          ? el("button", {
              class: "cap-del",
              type: "button",
              text: "×",
              title: t("删除技能「{name}」", { name: skill.name }),
              onclick: () => onRemove(skill),
            })
          : document.createDocumentFragment(),
      ),
      el("p", { class: "cap-desc", text: skill.description }),
      capMeta([
        skill.allowModelInvocation ? null : t("只走 $名字 显式引用"),
        skill.schedule !== null
          ? t("定时 {schedule}", { schedule: skill.schedule })
          : null,
        missing.length > 0
          ? t("依赖缺失：{list}", { list: missing.join(", ") })
          : null,
      ]),
    );
    fragment.append(row);
  }

  fragment.append(
    el("p", {
      class: "cap-note",
      text: t(
        "内置技能只读；自己加在 <home>/skills/ 下，下一轮就生效，不用重启。",
      ),
    }),
  );
  return fragment;
}

/** The MCP connector list: what Jeeves is wired to, and how each one authenticates. */
export function renderMcp(
  view: McpView,
  onAdd: () => void,
  onRemove: (server: McpRow) => void,
): DocumentFragment {
  const fragment = document.createDocumentFragment();
  fragment.append(
    capHead(
      t("MCP 连接器 · {length} 个", { length: view.servers.length }),
      t("＋ 添加连接器"),
      onAdd,
    ),
  );

  if (view.error !== null) {
    fragment.append(el("div", { class: "failure", text: view.error }));
  }
  if (view.servers.length === 0) {
    fragment.append(
      muted(
        t(
          "还没有连接器。连上一个 MCP 服务器，它的工具就并进 Jeeves 的工具表（<服务器>_<工具>）。",
        ),
      ),
    );
  }

  for (const server of view.servers) {
    const target =
      server.kind === "url"
        ? (server.url ?? "")
        : [server.command ?? "", ...server.args].join(" ").trim();
    const auth: Record<string, string> = {
      none: t("无鉴权"),
      auth_env: t("鉴权：{env}（环境变量，值以 Bearer 发出）", {
        env: server.authEnv ?? "",
      }),
      oauth: t("浏览器 OAuth（jeeves mcp login）"),
    };
    fragment.append(
      el(
        "div",
        {
          class: `cap-row${server.source === "builtin" ? " builtin" : ""}`,
          title:
            server.source === "builtin"
              ? t(
                  "「{name}」由系统内置配置提供（JEEVES_MCP_CONFIG）—— 只能看，不能改",
                  { name: server.name },
                )
              : `「${server.name}」 · ${target}`,
        },
        el(
          "div",
          { class: "cap-row-head" },
          el("span", { class: "cap-name", text: server.name }),
          el("span", {
            class: "tag",
            text: server.kind === "url" ? "HTTP" : "stdio",
          }),
          server.source === "builtin"
            ? el("span", { class: "tag cap-builtin-tag", text: t("内置") })
            : document.createDocumentFragment(),
          server.source === "user"
            ? el("button", {
                class: "cap-del",
                type: "button",
                text: "×",
                title: t("删除连接器「{name}」", { name: server.name }),
                onclick: () => onRemove(server),
              })
            : document.createDocumentFragment(),
        ),
        el("code", { class: "cap-target", text: target }),
        capMeta([auth[server.auth] ?? server.auth]),
      ),
    );
  }

  fragment.append(
    el("p", {
      class: "cap-note",
      text: t(
        "内置配置只读；改动落在 <home>/mcp.json，重启 Jeeves 后生效 —— 连接发生在启动时。",
      ),
    }),
  );
  return fragment;
}

/** Panel header: the title with a count, and the one primary action. */
function capHead(
  title: string,
  action: string,
  onAdd: () => void,
): HTMLElement {
  return el(
    "div",
    { class: "cap-head" },
    el("p", { class: "hint", text: title }),
    el("button", {
      class: "approve",
      type: "button",
      text: action,
      onclick: onAdd,
    }),
  );
}

/** The small tag row under a row's name. Null entries are dropped, not rendered empty. */
function capMeta(items: (string | null)[]): HTMLElement {
  const row = el("div", { class: "cap-meta" });
  for (const item of items) {
    if (item === null || item === "") continue;
    row.append(el("span", { class: "cap-meta-item", text: item }));
  }
  return row;
}

/**
 * The scheduled-jobs list: when it fires, what it does, and where the result goes.
 *
 * A row that came from a skill's `schedule:` line is greyed and has no delete button — the
 * job **is** that skill, and removing it here would either do nothing or lie. The "From a skill"
 * tag plus the title attribute say where to go instead.
 */
export function renderSchedules(
  view: SchedulesView,
  onAdd: () => void,
  onRemove: (job: ScheduleJob) => void,
): DocumentFragment {
  const fragment = document.createDocumentFragment();
  fragment.append(
    capHead(
      t("定时任务 · {length} 个", { length: view.jobs.length }),
      t("＋ 添加任务"),
      onAdd,
    ),
  );

  if (view.error !== null) {
    fragment.append(el("div", { class: "failure", text: view.error }));
  }
  if (view.jobs.length === 0) {
    fragment.append(
      muted(
        t(
          "还没有定时任务。加一个：什么时候（cron，如 0 8 * * 1-5）、做什么（一句话）。到点它会在自己的会话里跑一轮，结果写进 outbox。",
        ),
      ),
    );
  }

  for (const job of view.jobs) {
    const row = el(
      "div",
      {
        class: `cap-row${job.source === "skill" ? " builtin" : ""}`,
        title:
          job.source === "skill"
            ? t(
                "「{name}」的定时来自技能 {skill} 的 SKILL.md —— 改那里，不在这里",
                {
                  name: job.name,
                  skill: job.skill ?? job.name,
                },
              )
            : t("「{name}」存在状态库里", { name: job.name }),
      },
      el(
        "div",
        { class: "cap-row-head" },
        el("span", { class: "cap-name", text: job.name }),
        el("span", {
          class: "tag sched-cron",
          text:
            job.runAt !== null
              ? t("一次 {runAt}", { runAt: readableMinute(job.runAt) })
              : job.cron,
          title: job.runAt ?? job.cron,
        }),
        job.source === "skill"
          ? el("span", { class: "tag cap-builtin-tag", text: t("随技能") })
          : document.createDocumentFragment(),
        job.source === "store"
          ? el("button", {
              class: "cap-del",
              type: "button",
              text: "×",
              title: t("删除定时任务「{name}」", { name: job.name }),
              onclick: () => onRemove(job),
            })
          : document.createDocumentFragment(),
      ),
      el("p", { class: "cap-desc", text: job.prompt }),
      capMeta([
        job.nextRun !== null
          ? t("下次 {nextRun}", { nextRun: readableMinute(job.nextRun) })
          : job.nextRunNote,
        job.lastFiredMinute !== null
          ? t("上次 {lastFiredMinute}", {
              lastFiredMinute: readableMinute(job.lastFiredMinute),
            })
          : t("还没跑过"),
        job.claimedMinute !== null
          ? t("上次被中断，重启时按「错过是否补跑」处理")
          : null,
        t("投递 {delivery}", { delivery: job.delivery }),
        job.catchUp ? null : t("错过不补跑"),
        job.runAt !== null ? t("跑完自动删除") : null,
      ]),
    );
    fragment.append(row);
  }

  fragment.append(
    el("p", {
      class: "cap-note",
      text: t(
        "到点由常驻的 Runtime 触发 —— 它没在运行就不会触发。输出落在 <home>/outbox/，不进聊天。",
      ),
    }),
  );
  return fragment;
}


/** Local `HH:MM` from the ledger's ISO stamp; the raw value when it will not parse. */
function clockOf(iso: string): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return iso;
  return date.toLocaleTimeString("zh-CN", {
    hour: "2-digit",
    minute: "2-digit",
  });
}

/**
 * The ledger: one row per turn, opening in place.
 *
 * A folded row is marked, not hidden: after the backfill those turns genuinely only have a
 * summary, and pretending they are complete would make the ledger lie about its own fidelity.
 */
export function renderLedger(
  turns: LedgerTurnView[],
  entries: LedgerEntryView[],
  openTurn: string | null,
  onOpen: (turnId: string) => void,
): DocumentFragment {
  const fragment = document.createDocumentFragment();
  if (turns.length === 0) {
    fragment.append(
      muted(
        t(
          "这个会话还没有记录。第一轮说完话，账本就会有第一行 —— 只追加，不改写。",
        ),
      ),
    );
    return fragment;
  }
  const head = el(
    "div",
    { class: "mem-head" },
    el("p", {
      class: "hint",
      text: t("账本 · {length} 轮", { length: turns.length }),
    }),
    el("span", { class: "mem-note", text: t("只追加 · 可重建") }),
  );
  fragment.append(head);

  for (const turn of turns) {
    const folded = turn.fidelity !== "exact";
    const row = el("button", {
      class: `mem-row${openTurn === turn.turnId ? " open" : ""}`,
      type: "button",
      title: folded
        ? t("这一轮的工具输出只保留了摘要（回填而来，原文已不可重建）")
        : turn.preview,
      onclick: () => onOpen(turn.turnId),
    });
    row.append(
      el("span", { class: "mem-time", text: clockOf(turn.at) }),
      el("span", {
        class: "mem-preview",
        text: turn.preview === "" ? t("（空）") : turn.preview,
      }),
      el(
        "span",
        { class: "mem-tags" },
        el("span", {
          class: "tag",
          text: t("{entries} 条", { entries: turn.entries }),
        }),
        turn.consolidated
          ? el("span", { class: "tag", text: t("已整理") })
          : document.createDocumentFragment(),
        folded
          ? el("span", { class: "tag mem-folded", text: t("折叠") })
          : document.createDocumentFragment(),
      ),
    );
    fragment.append(row);

    if (openTurn !== turn.turnId) continue;
    const detail = el("div", { class: "mem-entries" });
    if (entries.length === 0) {
      detail.append(muted(t("这一轮没有可展开的条目。")));
    }
    for (const entry of entries) {
      detail.append(
        el(
          "div",
          { class: "mem-entry" },
          el("span", {
            class: `mem-kind mem-kind-${entry.kind}`,
            text: entry.kind,
          }),
          el("span", { class: "mem-entry-text", text: entry.preview }),
          entry.fidelity !== "exact"
            ? el("span", { class: "tag mem-folded", text: t("折叠") })
            : document.createDocumentFragment(),
        ),
      );
    }
    fragment.append(detail);
  }
  return fragment;
}

/**
 * The bundle: catalog on the left, one page on the right, provenance across the top.
 *
 * `sources` are clickable on purpose — the whole point of a derived knowledge page is that
 * you can walk back to the rows it was distilled from.
 */
export function renderWiki(
  pages: MemoryPageSummary[],
  current: MemoryPageView | null,
  onPick: (slug: string) => void,
  onSource: (source: string) => void,
  onRebuild: () => void,
): DocumentFragment {
  const fragment = document.createDocumentFragment();
  if (pages.length === 0) {
    fragment.append(
      muted(
        t(
          "记忆库还是空的。它由后台整理写出来 —— 攒够几轮之后就会有自己的目录。",
        ),
      ),
    );
    return fragment;
  }

  const catalog = el("div", { class: "mem-catalog" });
  catalog.append(
    el("p", {
      class: "hint",
      text: t("目录 · {length} 页", { length: pages.length }),
    }),
  );
  for (const page of pages) {
    catalog.append(
      el(
        "button",
        {
          class: `mem-page-row${current !== null && current.slug === page.slug ? " open" : ""}`,
          type: "button",
          title: page.summary,
          onclick: () => onPick(page.slug),
        },
        el("span", { class: "mem-page-title", text: page.title }),
        el("span", { class: "tag", text: page.kind }),
      ),
    );
  }
  fragment.append(catalog);

  if (current === null) {
    fragment.append(
      muted(t("点左边的一页看看 —— 每页都带着它从哪几轮账本里来的。")),
    );
    return fragment;
  }

  const sources = el("div", { class: "mem-sources" });
  if (current.sources.length === 0) {
    sources.append(
      el("span", { class: "mem-note", text: t("这页没有记出处") }),
    );
  }
  for (const source of current.sources) {
    sources.append(
      el("button", {
        class: "chip note mem-source",
        type: "button",
        text: source,
        title: t("跳到账本里那一轮"),
        onclick: () => onSource(source),
      }),
    );
  }

  fragment.append(
    el(
      "article",
      { class: "mem-page" },
      el(
        "header",
        { class: "mem-page-head" },
        el("h1", { class: "mem-page-name", text: current.title }),
        el("span", { class: "tag", text: current.kind }),
      ),
      el("p", { class: "mem-page-summary", text: current.summary }),
      sources,
      el("div", { class: "mem-page-body" }, renderMarkdown(current.body)),
      el("button", {
        class: "ghost mem-rebuild",
        type: "button",
        text: t("重新生成"),
        title: t("换更强的模型后可以整页重跑（会先在后台重读来源轮次）"),
        onclick: onRebuild,
      }),
    ),
  );
  return fragment;
}

/**
 * The context meter: one band, six sections, two numbers underneath.
 *
 * A notice appears at the configured threshold — a sentence at the end of the bar, not a
 * dialog: being close to the window's edge is information, not an emergency.
 */
export function renderContext(
  usage: ContextUsage | null,
  onCompact: () => void,
): DocumentFragment {
  const fragment = document.createDocumentFragment();
  if (usage === null) {
    fragment.append(muted(t("还没有测量 —— 这个会话跑过一轮之后就有数了。")));
    return fragment;
  }
  const total = usage.parts.reduce(
    (sum, part) => sum + Math.max(part.tokens, 0),
    0,
  );
  const band = el("div", {
    class: "mem-band",
    role: "img",
    "aria-label": t("上下文占用分层条"),
  });
  for (const part of usage.parts) {
    if (part.tokens <= 0 || total === 0) continue;
    band.append(
      el("span", {
        class: `mem-band-seg seg-${part.name}`,
        style: `flex-grow: ${part.tokens}`,
        title: `${part.name} · ${part.tokens} token`,
      }),
    );
  }

  const percent = Math.round(usage.ratio * 100);
  const numbers = el(
    "div",
    { class: "mem-numbers" },
    el("span", {
      text: `${usage.usedTokens} / ${usage.budgetTokens} token · ${percent}%`,
    }),
    el(
      "span",
      { class: `mem-percent${percent >= 85 ? " hot" : ""}` },
      el("span", { class: "mem-key" }),
      el("span", {
        text: t("灵魂 · 记忆 · 技能 · 工具 · 历史 · 摘要（hover 看明细）"),
      }),
    ),
  );

  fragment.append(
    el(
      "div",
      { class: "mem-meter" },
      el("p", { class: "hint", text: t("上下文占用") }),
      band,
      numbers,
      usage.notice !== null
        ? el("p", { class: "mem-warn", text: usage.notice })
        : document.createDocumentFragment(),
      el("button", {
        class: "ghost mem-compact",
        type: "button",
        text: t("现在压缩"),
        title: t("把较早的轮次折进摘要，保留最近两轮"),
        onclick: onCompact,
      }),
    ),
  );
  return fragment;
}
