import { accountKey, accountStorage, hostedBase, loginPage } from "./hosted.ts";
import { setupNavigation } from "./navigation";
import {
  webState,
  workspaceActions,
  updateControls,
  refreshPolicy,
  saveWorkspace,
} from "./controls";
import { errorText, preferenceRequest } from "./api";
import { webTasks } from "./tasks";
import { submissionTracker } from "./ui/submission.ts";
const submissions = submissionTracker(accountStorage);

import type {
  AgentTaskView,
  ContextUsage,
  LedgerEntryView,
  LedgerTurnView,
  MemoryPageSummary,
  MemoryPageView,
  RememberScope,
  TurnAttachment,
  TurnCompletedNotification,
  TurnView,
} from "./protocol";
import type {
  CapabilityView,
  McpRow,
  McpServerSpec,
  McpView,
  Outcome,
  ScheduleJob,
  ScheduleSpec,
  SchedulesView,
  SkillRow,
  SkillsView,
} from "./api";
import { applyStatic, getLang, initLang, t } from "./i18n";
import { createTimeline } from "./timeline";
import { taskCards } from "./task-card";
import { queuedTasks } from "./task-queued";
import { taskOperationForm, chatOperationForm } from "./task-operation";
import { taskInputForm } from "./task-input";
import { startSync } from "./sync";
import { catchUpTaskEvents } from "./tasks";
import {
  compactSession,
  createFolder,
  fetchContextUsage,
  fetchDashboard,
  fetchCapabilities,
  fetchFs,
  fetchLedger,
  fetchLedgerTurn,
  fetchMemoryIndex,
  fetchMemoryPage,
  fetchSkillStats,
  fetchMcp,
  fetchMessages,
  fetchSchedules,
  fetchRecents,
  fetchSessionMeta,
  fetchSkills,
  fetchTurns,
  followUpTurn,
  installSkill,
  interruptTurn,
  recordRecent,
  removeMcpServer,
  removeSchedule,
  removeSkill,
  respondApproval,
  saveMcpServer,
  saveSchedule,
  steerTurn,
  interact,
  taskAction,
  uploadFile,
} from "./api";
import { renderMarkdown, StreamMarkdown } from "./markdown";
import {
  attachmentStrip,
  chip,
  el,
  elapsedSince,
  failure,
  failureOf,
  formatMs,
  messageRow,
  taskNotice,
  muted,
  renderCapabilities,
  renderContext,
  renderLedger,
  renderMcp,
  renderSchedules,
  renderOverview,
  renderSkills,
  renderWiki,
  toolSummary,
  turnFooter,
  welcome,
} from "./render";

/**
 * Which session is currently being viewed.
 *
 * A session is not *stored* in the browser — it is just a label on chat_log, and the server
 * owns the rows — but which one you are reading is client state, and a reload should not
 * silently take you somewhere else. It is remembered in localStorage next to the workspace;
 * `boot()` drops it if the list no longer has that id.
 *
 * `default` is still the first-run answer: `jeeves` in the terminal talks on the same line, so
 * a fresh browser and a fresh terminal meet in the same conversation.
 */
const SESSION_KEY = "jeeves.session";
let current = accountStorage.getItem(SESSION_KEY) ?? "default";
const startupDraftKey = accountKey("jeeves.chat-draft.preparing");
let preparing = false;

/** Ids the last `refresh()` saw, newest first. Boot uses this to tell "the remembered
 *  session still exists" from "it was deleted since" — opening a session the list no longer
 *  shows would paint an empty shell with no way back except the list. */

/** Switch the label *and* remember it. Every path that changes `current` goes through here. */
function rememberSession(id: string): void {
  current = id;
  if (preparing) {
    const draft = sessionStorage.getItem(startupDraftKey);
    if (draft !== null) {
      saveDraft(draft);
      sessionStorage.removeItem(startupDraftKey);
    }
    preparing = false;
    dom.send.disabled = false;
  }
  dom.input.value = sessionStorage.getItem(accountKey("jeeves.chat-draft." + id)) ?? "";
  dom.input.disabled = false;
  accountStorage.setItem(SESSION_KEY, id);
}

function saveDraft(text: string): void {
  const key = accountKey("jeeves.chat-draft." + current);
  if (text) sessionStorage.setItem(key, text);
  else sessionStorage.removeItem(key);
}

/**
 * Where each conversation was left, by session id.
 *
 * Distance **from the bottom**, not an absolute offset: older pages get loaded, images size
 * themselves, and the top edge moves — the newest edge does not. Deliberately in memory
 * only: "switch back to where I was reading" is about moving between conversations in one
 * sitting, while a reload should land on the newest message (the session itself is
 * remembered in localStorage; the reading position is not).
 */
const reading = new Map<string, number>();

/** Cursor for paging back through older messages. `null` = already at the earliest. */
let olderCursor: string | null = null;
let historySession = "";
let newestHistoryId = 0;
let readingNotices = false;


/** How often the panels refresh themselves. All local RPC, a few microseconds, no cost. */
const POLL_MS = 15_000;

const dom = {
  messages: must("messages"),
  jumpLatest: must<HTMLButtonElement>("jump-latest"),
  composer: must<HTMLFormElement>("composer"),
  input: must<HTMLTextAreaElement>("input"),
  send: must<HTMLButtonElement>("send"),
  /** Only shown while a turn can be followed: pressing it means this lands as the next turn,
   *  not as an addition to the current one. */
  queueToggle: must<HTMLButtonElement>("queue-toggle"),
  older: must<HTMLButtonElement>("older"),
  overview: must("overview"),
  status: must("status"),
  /** A button, never an input: the human picks a folder, and that pick is the write boundary. */

  progress: must<HTMLButtonElement>("progress"),
  chatOperations: must<HTMLButtonElement>("chat-operations"),
  file: must<HTMLInputElement>("file"),
  staged: must<HTMLElement>("staged"),
  session: must("session"),
  drawer: must("drawer"),
  drawerClose: must<HTMLButtonElement>("drawer-close"),
  panelStash: must("panel-stash"),
  panelCapabilities: must("panel-capabilities"),
  panelSkills: must("panel-skills"),
  panelMcp: must("panel-mcp"),
  panelMemory: must("panel-memory"),
  panelSchedule: must("panel-schedule"),
  capabilitiesBody: must("capabilities-body"),
  skillsBody: must("skills-body"),
  mcpBody: must("mcp-body"),
  schedulesBody: must("schedules-body"),
  memSegLedger: must<HTMLButtonElement>("mem-seg-ledger"),
  memSegWiki: must<HTMLButtonElement>("mem-seg-wiki"),
  memSegContext: must<HTMLButtonElement>("mem-seg-context"),
  memLedger: must("mem-ledger"),
  memWiki: must("mem-wiki"),
  memContext: must("mem-context"),
};

/**
 * The chosen workspace. One of two things this page keeps in localStorage (the other is
 * which session is open): a person expects a folder picker to remember the folder, and the
 * server already records it in workspaces.json for the "recents" list. Never a path typed
 * by hand.
 */
const WORKSPACE_KEY = "jeeves.workspace";
let workspace: string | null = accountStorage.getItem(WORKSPACE_KEY);
/** Where uploads land. Kept off-screen (config is not the point of this page), but needed to
 *  tell the model where to find a non-image attachment. */
let jeevesHome = "";

function setWorkspace(path: string | null): void {
  workspace = path;
  webState.cwd = path ?? undefined;
  if (path === null) accountStorage.removeItem(WORKSPACE_KEY);
  else accountStorage.setItem(WORKSPACE_KEY, path);
}

let busy = false;

/**
 * A turn running on **this session, but not on this page** — another tab, the terminal,
 * or a follow-up queued by an earlier turn. `busy` cannot see it: no stream lives here.
 * So the page asks `turn/list`, on open and on every pulse. Its value is that turn's
 * `startedAt`, which lets the statusline say "running for 3 minutes" instead of "running".
 *
 * Sending while this is set still means **steer**: there is a turn to attach to, and
 * asking for a second one would be refused (`SESSION_BUSY`).
 */
let otherTurn: string | null = null;

/** This is the "say it after this turn" wording: while a turn is running, sending lands as
 *  the **next** turn instead of adding to the current one. */
let queuedMode = false;

/**
 * Staging area ("reference first"): files wait here, uploaded on send with absolute paths
 * injected into the message — referencing and reading are two things; the model decides.
 */
const staged: File[] = [];

/** Object URLs for staged image previews; revoked the moment they leave the stage. */
const stagedUrls = new Map<File, string>();

function previewUrl(file: File): string | undefined {
  if (!file.type.startsWith("image/")) return undefined;
  let url = stagedUrls.get(file);
  if (url === undefined) {
    url = URL.createObjectURL(file);
    stagedUrls.set(file, url);
  }
  return url;
}

function releaseStaged(files: File[]): void {
  for (const file of files) {
    const url = stagedUrls.get(file);
    if (url !== undefined) {
      URL.revokeObjectURL(url);
      stagedUrls.delete(file);
    }
  }
}

function renderStaged(): void {
  dom.staged.hidden = staged.length === 0;
  const nodes = staged.map((file, index) => {
    const url = previewUrl(file);
    if (url !== undefined) {
      const img = el("img", {
        class: "attach-thumb",
        src: url,
        alt: file.name,
        title: file.name,
      });
      img.onerror = () => img.remove();
      const drop = el("button", {
        class: "thumb-drop",
        text: "×",
        title: t("拿掉这个文件"),
        type: "button",
      });
      drop.addEventListener("click", () => {
        releaseStaged([file]);
        staged.splice(index, 1);
        renderStaged();
      });
      return el("span", { class: "thumb" }, img, drop);
    }
    const item = el("span", { class: "chip", text: file.name });
    const drop = el("button", {
      class: "ghost",
      text: "×",
      title: t("拿掉这个文件"),
    });
    drop.addEventListener("click", () => {
      releaseStaged([file]);
      staged.splice(index, 1);
      renderStaged();
    });
    item.append(drop);
    return item;
  });
  dom.staged.replaceChildren(...nodes);
}

/**
 * The drawer holds only the conversation overview. Skills, MCP, memory and schedules are
 * things a person manages now and then, so they live in Settings: its tab borrows the panel
 * from the stash while it shows, and hands it back when it closes.
 */
type PanelId = "capabilities" | "skills" | "mcp" | "memory" | "schedule";

/** Latest capability data, refreshed with the rest of the screen. */
let capabilitiesView: CapabilityView | null = null;
let skillsView: SkillsView = { skills: [], error: null };
let mcpView: McpView = { servers: [], error: null };
let schedulesView: SchedulesView = { jobs: [], error: null };

function panelOf(id: PanelId): HTMLElement {
  return { capabilities: dom.panelCapabilities, skills: dom.panelSkills, mcp: dom.panelMcp, memory: dom.panelMemory, schedule: dom.panelSchedule }[id];
}

/** Paint the read-only overview from the latest inventory. */
function paintCapabilities(): void {
  if (capabilitiesView === null) return;
  dom.capabilitiesBody.replaceChildren(renderCapabilities(capabilitiesView));
}

/** Move one panel into `host`, paint it, and return the call that puts it back. */
function mountPanel(id: PanelId, host: HTMLElement): () => void {
  const panel = panelOf(id);
  host.append(panel);
  panel.hidden = false;
  if (id === "capabilities") paintCapabilities();
  if (id === "skills") {
    paintSkills();
    void loadSkillStats();
  }
  if (id === "mcp") paintMcp();
  if (id === "schedule") paintSchedules();
  if (id === "memory") void loadMemory();
  return () => {
    panel.hidden = true;
    dom.panelStash.append(panel);
  };
}

/** The overview drawer slides in from the right; the chat column never squeezes. */
function openDrawer(): void {
  dom.drawer.hidden = false;
}

function closeDrawer(): void {
  dom.drawer.hidden = true;
}

/**
 * How often each skill actually fired — the memory center's answer to "is this skill worth
 * keeping". Telemetry: if the log cannot be read, the panel simply says nothing about it.
 */
let skillHits = new Map<string, number>();

async function loadSkillStats(): Promise<void> {
  try {
    const answer = await fetchSkillStats();
    skillHits = new Map(answer.data.map((stat) => [stat.name, stat.hits]));
  } catch {
    return;
  }
  paintSkills();
}

/** The three most-fired skills, as one line. Silent when nothing has fired yet. */
function hitsLine(): Node[] {
  const rows = [...skillHits.entries()]
    .filter(([, hits]) => hits > 0)
    .sort((left, right) => right[1] - left[1])
    .slice(0, 3);
  if (rows.length === 0) return [];
  const text = rows
    .map(([name, hits]) => t("{name} {hits} 次", { name: name, hits: hits }))
    .join("、");
  return [chip(t("命中：{text}", { text: text }), "note")];
}

/** Paint the skill panel from the latest `skillsView`. */
function paintSkills(): void {
  const note = outcomeNote(dom.skillsBody);
  dom.skillsBody.replaceChildren(
    ...(note === null ? [] : [note]),
    ...hitsLine(),
    renderSkills(
      skillsView,
      () => void skillAddOverlay(),
      (skill) => void removeSkillAsk(skill),
    ),
  );
}

/** Paint the MCP panel from the latest `mcpView`. */
function paintMcp(): void {
  const note = outcomeNote(dom.mcpBody);
  dom.mcpBody.replaceChildren(
    ...(note === null ? [] : [note]),
    renderMcp(
      mcpView,
      () => void mcpAddOverlay(),
      (server) => void removeMcpAsk(server),
    ),
  );
}

/** Paint the scheduled-jobs panel from the latest `schedulesView`. */
function paintSchedules(): void {
  const note = outcomeNote(dom.schedulesBody);
  dom.schedulesBody.replaceChildren(
    ...(note === null ? [] : [note]),
    renderSchedules(
      schedulesView,
      () => void scheduleAddOverlay(),
      (job) => removeScheduleAsk(job),
    ),
  );
}


const MEM_SEGS = ["ledger", "wiki", "context"] as const;
type MemSeg = (typeof MEM_SEGS)[number];

let memSeg: MemSeg = "ledger";
let ledgerTurns: LedgerTurnView[] = [];
let ledgerEntries: LedgerEntryView[] = [];
/** Which turn is open. `null` = all rows collapsed. */
let ledgerOpen: string | null = null;
let wikiPages: MemoryPageSummary[] = [];
let wikiCurrent: MemoryPageView | null = null;
let contextUsage: ContextUsage | null = null;
/** One line of context for the current view (a rebuild hint, a "nothing to fold"). */
let memNote: string | null = null;
let memError: unknown = null;

function paintMemory(): void {
  for (const [id, button] of [
    ["ledger", dom.memSegLedger],
    ["wiki", dom.memSegWiki],
    ["context", dom.memSegContext],
  ] as const) {
    const active = id === memSeg;
    button.classList.toggle("active", active);
    button.setAttribute("aria-selected", String(active));
  }
  dom.memLedger.hidden = memSeg !== "ledger";
  dom.memWiki.hidden = memSeg !== "wiki";
  dom.memContext.hidden = memSeg !== "context";

  if (memError !== null) {
    const error = memError;
    memError = null;
    const target =
      memSeg === "ledger"
        ? dom.memLedger
        : memSeg === "wiki"
          ? dom.memWiki
          : dom.memContext;
    target.replaceChildren(failureOf(error));
    return;
  }

  const note = memNote === null ? [] : [chip(memNote, "note")];
  if (memSeg === "ledger") {
    dom.memLedger.replaceChildren(
      renderLedger(
        ledgerTurns,
        ledgerEntries,
        ledgerOpen,
        (turnId) => void openLedgerTurn(turnId),
      ),
    );
  } else if (memSeg === "wiki") {
    dom.memWiki.replaceChildren(
      ...note,
      renderWiki(
        wikiPages,
        wikiCurrent,
        (slug) => void openMemoryPage(slug),
        (source) => void jumpToSource(source),
        rebuildHint,
      ),
    );
  } else {
    dom.memContext.replaceChildren(
      renderContext(contextUsage, () => void compactNow()),
    );
  }
}

/** Read the view that is showing, then paint it. */
async function loadMemory(): Promise<void> {
  if (current === "") {
    memError = new Error(t("还没有选会话 —— 先开一个会话，账本才有得看。"));
    paintMemory();
    return;
  }
  try {
    if (memSeg === "ledger") {
      const answer = await fetchLedger(current, 30);
      ledgerTurns = answer.turns;
      ledgerEntries = [];
      ledgerOpen = null;
    } else if (memSeg === "wiki") {
      const answer = await fetchMemoryIndex();
      wikiPages = answer.pages;
      wikiCurrent = null;
    } else {
      const answer = await fetchContextUsage(current);
      contextUsage = answer.usage;
    }
    memError = null;
  } catch (error) {
    memError = error;
  }
  paintMemory();
}

/** Open one turn in place; opening the open one closes it. */
async function openLedgerTurn(turnId: string): Promise<void> {
  if (ledgerOpen === turnId) {
    ledgerOpen = null;
    ledgerEntries = [];
    paintMemory();
    return;
  }
  try {
    const answer = await fetchLedgerTurn(current, turnId);
    ledgerTurns = answer.turns;
    ledgerEntries = answer.entries;
    ledgerOpen = turnId;
    memError = null;
  } catch (error) {
    memError = error;
  }
  paintMemory();
}

async function openMemoryPage(slug: string): Promise<void> {
  try {
    const answer = await fetchMemoryPage(slug);
    if (answer.page === null) {
      memNote = t(
        "记忆库里没有「{slug}」这一页 —— 它可能在后台整理时被改名了，刷新目录看看。",
        { slug: slug },
      );
      wikiCurrent = null;
    } else {
      wikiCurrent = answer.page;
      memNote = null;
    }
    memError = null;
  } catch (error) {
    memError = error;
  }
  paintMemory();
}

/**
 * Walk back from a page's provenance to the ledger rows it came from.
 *
 * Sources read `ledger:<session>/<turn>` (older pages: `ledger:<seq>`). Neither form is a
 * session-scoped `turnId` we can open directly, so the honest move is: switch to the ledger
 * and open the row whose id ends with that tail — or say we could not find it.
 */
async function jumpToSource(source: string): Promise<void> {
  const tail = source.split(/[:/]/).filter(Boolean).at(-1) ?? "";
  memSeg = "ledger";
  memNote = null;
  if (tail === "") {
    await loadMemory();
    return;
  }
  try {
    const answer = await fetchLedger(current, 100);
    ledgerTurns = answer.turns;
    const match = answer.turns.find(
      (turn) => turn.turnId === tail || turn.turnId.endsWith(tail),
    );
    if (match === undefined) {
      ledgerEntries = [];
      ledgerOpen = null;
      memNote = t(
        "账本里没有 {source} 对应的那一轮（可能已被整理进更早的摘要）。",
        { source: source },
      );
      memError = null;
      paintMemory();
      return;
    }
    await openLedgerTurn(match.turnId);
  } catch (error) {
    memError = error;
    paintMemory();
  }
}

/**
 * "Regenerate" is a **command**, not a button that secretly spends a model call.
 *
 * Whole-page regeneration re-reads the source turns with whatever model is configured now;
 * that is `jeeves memory rebuild`, and it belongs where a person can see the cost.
 */
function rebuildHint(): void {
  memNote =
    t("整页重生成走 CLI：jeeves memory rebuild --page ") +
    (wikiCurrent?.slug ?? "<slug>");
  paintMemory();
}

/** `POST /api/compact`: fold the older turns in now, keeping the newest few. */
async function compactNow(): Promise<void> {
  try {
    const answer = await compactSession(current);
    memNote =
      answer.coveredTurns === 0
        ? t("没有更早的轮次可折 —— 摘要已经跟上了。")
        : t(
            "已压缩：摘要现在覆盖 {coveredTurns} 轮（{summaryTokens} token）。",
            {
              coveredTurns: answer.coveredTurns,
              summaryTokens: answer.summaryTokens,
            },
          );
    const usage = await fetchContextUsage(current);
    contextUsage = usage.usage;
    memError = null;
  } catch (error) {
    memError = error;
  }
  paintMemory();
}

/**
 * The last write's outcome on a panel, remembered so the 15s repaint cannot wipe it.
 *
 * A one-shot prepend would vanish on the next poll — "did the delete work?" answered
 * by a line that disappears is worse than no line. Keyed by the panel element, so the
 * two lists never borrow each other's receipt.
 */
const lastOutcome = new WeakMap<
  HTMLElement,
  { ok: true; message: string } | { ok: false; error: unknown }
>();

function outcomeNote(panel: HTMLElement): Node | null {
  const outcome = lastOutcome.get(panel);
  if (outcome === undefined) return null;
  return outcome.ok ? chip(outcome.message, "note") : failureOf(outcome.error);
}


let statusTimer: number | null = null;
let statusStart = 0;
/** The self-dismiss timer of the crash-recovery notice (see startSync's onActivity). */
let resumeNotice: number | null = null;
/** The running turn's handle (from `turnStarted`) — what the stop button points at. */
let activeTurnId: string | null = null;

function showStatus(kind: "thinking" | "working", label: string): void {
  dom.status.hidden = false;
  dom.status.className = `statusline ${kind}`;
  if (dom.status.querySelector(".status-label") === null) {
    statusStart = Date.now();
    dom.status.replaceChildren(
      el("span", { class: "status-label", text: label }),
      el("span", { class: "status-time" }),
    );
    statusTimer = window.setInterval(() => {
      const time = dom.status.querySelector(".status-time");
      if (time !== null) time.textContent = elapsedSince(statusStart);
    }, 500);
  } else {
    const labelNode = dom.status.querySelector(".status-label");
    if (labelNode !== null) labelNode.textContent = label;
  }
  if (dom.status.className !== `statusline ${kind}`) {
    statusStart = Date.now();
    dom.status.className = `statusline ${kind}`;
  }
}

function hideStatus(): void {
  dom.status.hidden = true;
  dom.status.replaceChildren();
  activeTurnId = null;
  if (statusTimer !== null) {
    window.clearInterval(statusTimer);
    statusTimer = null;
  }
}

/** Full view for an image: click anywhere or Esc to leave — no chrome, just the picture. */
function openLightbox(src: string, alt: string): void {
  const overlay = el("div", {
    class: "lightbox",
    role: "dialog",
    "aria-label": alt,
  });
  overlay.append(el("img", { src, alt }));
  overlay.addEventListener("click", () => overlay.remove());
  window.addEventListener(
    "keydown",
    (event) => {
      if (event.key === "Escape") overlay.remove();
    },
    { once: true },
  );
  document.body.append(overlay);
}

/** The stop button rides the status line while a turn runs — one click, honest answer. */
function addStopButton(): void {
  if (dom.status.querySelector(".status-stop") !== null) return;
  const stop = el("button", {
    class: "ghost status-stop",
    text: t("停止"),
    type: "button",
  });
  stop.addEventListener("click", () => {
    if (activeTurnId === null) return;
    stop.disabled = true;
    void interruptTurn({ turnId: activeTurnId })
      .then((result) => {
        if (!result.interrupted) {
          const label = dom.status.querySelector(".status-label");
          if (label !== null) label.textContent = t("（那一轮已经结束了）");
        }
      })
      .catch(() => undefined);
  });
  dom.status.append(stop);
}

/** Puts a starter into the composer, caret at the end, as if the person had typed it. */
function fillComposer(text: string): void {
  dom.input.value = text;
  dom.input.dispatchEvent(new Event("input", { bubbles: true }));
  dom.input.focus();
  dom.input.setSelectionRange(text.length, text.length);
}

async function waitForHostedWorkspace(): Promise<boolean> {
  const base = hostedBase();
  if (!base) {
    location.replace("/");
    return false;
  }
  preparing = true;
  dom.send.disabled = true;
  dom.input.disabled = false;
  dom.input.value = sessionStorage.getItem(startupDraftKey) ?? sessionStorage.getItem(accountKey("jeeves.chat-draft." + current)) ?? "";
  dom.status.hidden = false;
  dom.status.textContent = t("工作区正在启动，你可以先写下要交办的事。");
  for (let attempt = 0; attempt < 25; attempt++) {
    try {
      const response = await fetch(base + "/api/workspace", {
        cache: "no-store",
        signal: AbortSignal.timeout(5000),
      });
      if (response.status === 401) {
        location.replace(loginPage());
        return false;
      }
      if (response.ok) {
        const workspace = await response.json();
        webState.managed = workspace.managed === true;
        if (workspace.ready === true) {
          dom.status.hidden = true;
          return true;
        }
      }
    } catch (error) {
      console.debug("Workspace will retry after a connection error", error);
    }
    await new Promise(resolve => window.setTimeout(resolve, 2000));
  }
  const retry = document.createElement("button");
  retry.type = "button";
  retry.textContent = t("重试");
  retry.onclick = () => location.reload();
  dom.status.replaceChildren(t("工作区还在准备中。"), retry);
  return false;
}

async function boot(): Promise<void> {
  initLang();
  dom.file.addEventListener("change", () => {
    for (const file of dom.file.files ?? []) staged.push(file);
    dom.file.value = "";
    renderStaged();
  });
  applyStatic(document);
  dom.input.addEventListener("input", () => {
    if (preparing) sessionStorage.setItem(startupDraftKey, dom.input.value);
    else saveDraft(dom.input.value);
    dom.input.style.height = "auto";
    dom.input.style.height = `${Math.min(dom.input.scrollHeight, 180)}px`;
  });
  if (!(await waitForHostedWorkspace())) return;
  dom.older.addEventListener("click", () => void older());
  dom.messages.addEventListener("scroll", updateJumpLatest, { passive: true });
  dom.jumpLatest.addEventListener("click", () => {
    pinToEnd();
    updateJumpLatest();
  });
  dom.chatOperations.addEventListener("click", () => {
    const session = current;
    const panel = chatOperationForm(
      session,
      () => {},
      () => close(),
      (error) => dom.messages.append(failureOf(error)),
    );
    const close = openOverlay(panel);
    panel.querySelector<HTMLButtonElement>("button:not(:disabled)")?.focus();
  });
  dom.drawerClose.addEventListener("click", closeDrawer);
  setupNavigation(closeDrawer);

  for (const area of [dom.messages, dom.staged]) {
    area.addEventListener("click", (event) => {
      const thumb =
        event.target instanceof Element
          ? event.target.closest(".attach-thumb")
          : null;
      if (thumb instanceof HTMLImageElement) openLightbox(thumb.src, thumb.alt);
    });
  }

  const barMore = must<HTMLDetailsElement>("bar-more");
  barMore.addEventListener("click", (event) => {
    if ((event.target as Element).closest("[role=menuitem]")) barMore.open = false;
  });
  document.addEventListener("click", (event) => {
    if (barMore.open && !barMore.contains(event.target as Node)) barMore.open = false;
  });
  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape" && barMore.open) barMore.open = false;
  });

  dom.progress.addEventListener("click", () => {
    if (dom.drawer.hidden) openDrawer();
    else closeDrawer();
  });

  for (const [button, seg] of [
    [dom.memSegLedger, "ledger"],
    [dom.memSegWiki, "wiki"],
    [dom.memSegContext, "context"],
  ] as const) {
    button.addEventListener("click", () => {
      memSeg = seg;
      memNote = null;
      void loadMemory();
    });
  }

  dom.composer.addEventListener("submit", (event) => {
    event.preventDefault();
    void submit();
  });

  dom.input.addEventListener("keydown", (event) => {
    if (event.key === "Enter" && !event.shiftKey && !event.isComposing) {
      event.preventDefault();
      void submit();
    }
  });

  dom.queueToggle.addEventListener("click", () => {
    queuedMode = !queuedMode;
    paintComposerMode();
    dom.input.focus();
  });


  refreshOnReturn();
  setInterval(() => {
    if (!busy) {
      void refresh();
      void syncTurns();
      void appendTaskNotices();
    }
  }, POLL_MS);

  startSync({
    onActivity: (ping) => {
      void catchUpTaskEvents();
      if (busy) return;
      if (ping.kind === "task_resumed") {
        dom.status.hidden = false;
        dom.status.className = "statusline resumed";
        dom.status.replaceChildren(
          el("span", {
            class: "status-label",
            text: t("上次中断的回合正在续跑（已完成的工具不重跑）"),
          }),
        );
        if (resumeNotice !== null) window.clearTimeout(resumeNotice);
        resumeNotice = window.setTimeout(hideStatus, 5000);
      }
      void refresh();
      if (current === webState.mainSessionId) void appendTaskNotices();
      if (ping.sessionId === current) void open(current);
    },
  });

  await refresh();
  await openMain();
  workspaceActions.main = openMain;
  setWorkspace(workspace);

  dom.input.focus();
}

/** Paint one screen of data. */
/** Pause / continue / cancel the current task — human switches, they exist nowhere else. */
async function switchTask(
  task: AgentTaskView,
  action:
    "start" | "wait" | "input" | "operations" | "suspend" | "resume" | "cancel",
): Promise<void> {
  if (action === "operations") {
    const panel = taskOperationForm(
      task,
      () => {
        void refresh();
      },
      () => close(),
      (error) => dom.overview.prepend(failureOf(error)),
    );
    const close = openOverlay(panel);
    panel.querySelector<HTMLButtonElement>("button:not(:disabled)")?.focus();
    return;
  }
  if (action === "wait" || action === "input") {
    const panel = taskInputForm(
      task,
      action,
      () => {
        const restore = panel.isConnected;
        close();
        void refresh().then(() => {
          const target =
            dom.overview.querySelector<HTMLButtonElement>(".taskcard button");
          if (
            restore &&
            document.activeElement === document.body &&
            target?.offsetParent
          )
            target.focus();
        });
      },
      () => close(),
      (error) => dom.overview.prepend(failureOf(error)),
    );
    const close = openOverlay(panel);
    panel.querySelector("textarea")?.focus();
    return;
  }
  if (
    action === "cancel" &&
    !window.confirm(
      t("取消任务「{title}」？取消后不会自动续跑。", { title: task.title }),
    )
  ) {
    return;
  }
  try {
    await taskAction(
      action === "start"
        ? {
            action,
            taskId: task.id,
            message: [task.title, task.goalCondition]
              .filter(Boolean)
              .join("\n"),
          }
        : { action, taskId: task.id },
    );
  } catch (error) {
    dom.overview.prepend(failureOf(error));
    return;
  }
  await refresh();
}

async function refresh(): Promise<void> {
  try {
    const data = await fetchDashboard(current);
    jeevesHome = data.config.home;
    updateControls(data, current);
    webState.busy = busy || otherTurn !== null;
    const selected = data.sessions.find((session) => session.id === current);
    dom.session.textContent =
      current === webState.mainSessionId
        ? "Jeeves"
        : selected?.messages
          ? selected.title
          : t("新会话");
    void refreshPolicy(current);
    dom.overview.replaceChildren(
      renderOverview(
        data,
        (task, action) => void switchTask(task, action),
      ),
    );
    if (lostMessage !== null && webState.message === lostMessage) webState.message = "";
    lostMessage = null;
  } catch (error) {
    webState.connected = false;
    lostMessage = connectionLost(error);
    webState.message = lostMessage;
    dom.overview.replaceChildren(failureOf(error));
  }
  await refreshCapabilities();
}

/** The banner text a failed refresh put up, so the next good refresh can take it down again. */
let lostMessage: string | null = null;

/**
 * Why the cockpit lost the workspace, for the banner. A failed fetch is the network or a
 * stopped gateway, not something the person did; it is worded as such and retried.
 */
function connectionLost(error: unknown): string {
  if (error instanceof TypeError || !navigator.onLine)
    return t("连不上 Jeeves，正在自动重试…");
  return errorText(error);
}

/** Refresh at once when the network or the tab comes back, instead of waiting for the poll. */
function refreshOnReturn(): void {
  window.addEventListener("online", () => {
    if (!busy) void refresh();
  });
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "visible" && !webState.connected && !busy) void refresh();
  });
}

/**
 * The capability center's two lists, in the same 15s pulse as everything else.
 *
 * Separate from `/api/data` on purpose: these are files on this machine, not protocol
 * reads, and a failure here must not blank the overview — the sidebar counts simply
 * keep their last value, and an open panel shows the error in place.
 */
async function refreshCapabilities(): Promise<void> {
  const [inventory, skills, mcp, schedules] = await Promise.all([
    fetchCapabilities().catch(() => null),
    fetchSkills().catch(() => null),
    fetchMcp().catch(() => null),
    fetchSchedules().catch(() => null),
  ]);
  if (inventory !== null) {
    capabilitiesView = inventory;
    if (!dom.panelCapabilities.hidden) paintCapabilities();
  }
  if (skills !== null) {
    skillsView = skills;
    if (!dom.panelSkills.hidden) paintSkills();
  }
  if (mcp !== null) {
    mcpView = mcp;
    if (!dom.panelMcp.hidden) paintMcp();
  }
  if (schedules !== null) {
    schedulesView = schedules;
    if (!dom.panelSchedule.hidden) paintSchedules();
  }
}

/**
 * Both composer entries browse the server's folders and persist to the selected session.
 */
async function cwdPicker(): Promise<void> {
  if (document.querySelector(".workspace-dialog")) return;
  const sessionId = webState.sessionId;
  const overlay = el("dialog", {
    class: "workspace-dialog",
    "aria-label": t("选择工作目录"),
  });
  const panel = el("div", { class: "fspanel" });
  overlay.append(panel);
  const close = () => {
    overlay.close();
    overlay.remove();
  };
  overlay.addEventListener("click", (event) => {
    if (event.target === overlay) close();
  });
  overlay.addEventListener("cancel", (event) => {
    event.preventDefault();
    close();
  });
  panel.append(el("p", { class: "hint", text: t("读目录中…") }));
  document.body.append(overlay);
  overlay.showModal();

  let directory = workspace ?? "";
  let showHidden = false;
  const recents = await fetchRecents().catch(() => [] as string[]);

  /** Every hop of the ancestor chain is clickable. Paths not starting with "/" degrade to text. */
  function crumbs(path: string): HTMLElement {
    const bar = el("div", { class: "fscrumbs" });
    if (!path.startsWith("/")) {
      bar.append(el("code", { class: "fscur", text: path }));
      return bar;
    }
    const parts = path.split("/").filter((segment) => segment !== "");
    const root = el("button", { class: "fsc", text: "/", type: "button" });
    root.addEventListener("click", () => void show("/"));
    bar.append(root);
    let prefix = "";
    for (const segment of parts) {
      prefix += `/${segment}`;
      const crumb = el("button", {
        class: "fsc",
        text: segment,
        type: "button",
      });
      const ancestor = prefix;
      crumb.addEventListener("click", () => void show(ancestor));
      bar.append(crumb);
    }
    return bar;
  }

  async function show(path: string | null): Promise<void> {
    panel.replaceChildren(el("p", { class: "hint", text: t("读目录中…") }));
    let listing: Awaited<ReturnType<typeof fetchFs>>;
    try {
      listing = await fetchFs(path, showHidden);
    } catch (error: unknown) {
      panel.replaceChildren(
        el("p", {
          class: "hint",
          text: t("读不动：{error}", { error: String(error) }),
        }),
        closeButton(),
      );
      return;
    }
    if (!overlay.isConnected) return;
    directory = listing.path;

    const list = el("div", { class: "fslist" });
    const here = listing.entries.map((entry) => entry.path);
    const shortcuts = recents.filter(
      (path) => path !== listing.path && !here.includes(path),
    );
    if (shortcuts.length > 0) {
      const block = el("div", { class: "fsrecent" });
      block.append(el("p", { class: "hint", text: t("最近使用") }));
      for (const path of shortcuts) {
        const row = el("button", {
          class: "fsrow",
          text: `🕒 ${path}`,
          type: "button",
        });
        row.addEventListener("click", () => void show(path));
        block.append(row);
      }
      list.append(block);
    }
    const rows = listing.entries.map((entry) => {
      const row = el("button", {
        class: "fsrow",
        text: `📁 ${entry.name}`,
        type: "button",
      });
      row.addEventListener("click", () => void show(entry.path));
      return row;
    });
    if (rows.length === 0 && shortcuts.length === 0) {
      list.append(muted(t("这层没有子目录了。要选的就是当前显示的目录。")));
    } else {
      list.append(...rows);
    }

    const mkdir = el("button", {
      class: "ghost",
      text: t("＋ 新建文件夹"),
      type: "button",
    });
    mkdir.addEventListener("click", () => {
      const name = window.prompt(
        t("在 {current} 下新建文件夹：", { current: directory }),
      );
      if (name === null || name.trim() === "") return;
      void createFolder(directory, name.trim())
        .then(() => show(directory))
        .catch((error: unknown) =>
          window.alert(t("建不了：{error}", { error: String(error) })),
        );
    });
    const hiddenToggle = el("button", {
      class: "ghost",
      text: showHidden ? t("隐藏点开头") : t("显示隐藏目录"),
      type: "button",
    });
    hiddenToggle.addEventListener("click", () => {
      showHidden = !showHidden;
      void show(directory);
    });

    const pick = el("button", {
      class: "approve",
      text: t("选这个目录"),
      type: "button",
    });
    pick.addEventListener("click", async () => {
      pick.disabled = true;
      if (!(await saveWorkspace(listing.path, sessionId))) {
        pick.disabled = false;
        return;
      }
      void recordRecent(listing.path).catch((error: unknown) => {
        webState.message = String(error);
      });
      close();
    });
    panel.replaceChildren(
      el("h3", { text: t("选择工作目录") }),
      el("p", { class: "hint", text: t("选择 Jeeves 服务所在设备的项目文件夹") }),
      crumbs(listing.path),
      list,
      el("div", { class: "row" }, pick, closeButton()),
      el("div", { class: "row" }, mkdir, hiddenToggle),
    );
    pick.focus();
  }

  function closeButton(): HTMLButtonElement {
    const cancel = el("button", {
      class: "ghost",
      text: t("取消"),
      type: "button",
    });
    cancel.addEventListener("click", () => {
      close();
    });
    return cancel;
  }

  await show(directory === "" ? null : directory);
}

/**
 * Run one capability write, then repaint and leave the outcome on the panel.
 *
 * The server's answer is a sentence a person can read (it refuses built-in rows with a
 * reason), so it is shown as-is — a silent success after a delete is exactly the kind
 * of "did that work?" this page avoids. Returns whether it succeeded, so a form knows
 * whether to close itself.
 */
async function capabilityWrite(
  action: () => Promise<Outcome>,
  panel: HTMLElement,
): Promise<boolean> {
  try {
    const outcome = await action();
    lastOutcome.set(panel, { ok: true, message: outcome.message });
    await refreshCapabilities();
    return true;
  } catch (error) {
    lastOutcome.set(panel, { ok: false, error });
    await refreshCapabilities();
    return false;
  }
}



function removeSkillAsk(skill: SkillRow): void {
  const ok = window.confirm(
    t("删除技能「{name}」？SKILL.md 会从 {folder} 移掉，下一轮起不再载入。", {
      name: skill.name,
      folder: skill.folder,
    }),
  );
  if (!ok) return;
  void capabilityWrite(() => removeSkill(skill.name), dom.skillsBody);
}


function removeMcpAsk(server: McpRow): void {
  const ok = window.confirm(
    t("删除 MCP 连接器「{name}」？重启 Jeeves 后不再连接它。", {
      name: server.name,
    }),
  );
  if (!ok) return;
  void capabilityWrite(() => removeMcpServer(server.name), dom.mcpBody);
}

/**
 * Mount `panel` as a modal overlay: backdrop click and Esc both close it.
 * Returns the close function (idempotent — removing twice is a no-op).
 */
function openOverlay(panel: HTMLElement): () => void {
  const previous = document.activeElement;
  const overlay = el("div", { class: "fsoverlay" });
  overlay.append(panel);
  (document.querySelector("dialog[open]") ?? document.body).append(overlay);
  let closed = false;
  const close = (): void => {
    if (closed) return;
    closed = true;

    overlay.remove();
    document.removeEventListener("keydown", onKey);
    if (previous instanceof HTMLElement && previous.isConnected)
      previous.focus();
  };
  const onKey = (event: KeyboardEvent): void => {
    if (event.key === "Escape") close();
  };
  overlay.addEventListener("click", (event) => {
    if (event.target === overlay) close();
  });
  document.addEventListener("keydown", onKey);
  return close;
}

/** A minimal SKILL.md, shown as the paste box's placeholder — the format at a glance. */
const SKILL_TEMPLATE = () =>
  getLang() === "zh"
    ? `---
name: my-skill
description: 什么时候用我 —— 触发词写在这里（英文，它会兼任触发器）
---

# 标题

第一步……
第二步……`
    : `---
name: my-skill
description: when to use me — trigger words go here (English; it doubles as the trigger)
---

# Title

First step…
Second step…`;

/**
 * Add a skill: paste a SKILL.md, or give a URL (GitHub blob / Gist pages are rewritten
 * to raw content server-side). Validation is the server's — the same gate
 * `jeeves skill install` passes — so a refusal comes back as a sentence on the panel.
 */
async function skillAddOverlay(): Promise<void> {
  const panel = el("div", {
    class: "fspanel cap-form",
    role: "dialog",
    "aria-label": t("添加技能"),
  });
  const text = el("textarea", {
    class: "cap-textarea",
    rows: 10,
    placeholder: SKILL_TEMPLATE(),
    "aria-label": t("SKILL.md 内容"),
  });
  const url = el("input", {
    type: "url",
    placeholder: t("https://…/SKILL.md（和粘贴框二选一）"),
    "aria-label": t("SKILL.md 网址"),
  });
  const close = openOverlay(panel);

  const install = el("button", {
    class: "approve",
    text: t("安装"),
    type: "button",
  });
  install.addEventListener("click", () => {
    install.disabled = true;
    void capabilityWrite(
      () => installSkill({ text: text.value, url: url.value.trim() }),
      dom.skillsBody,
    ).then((ok) => {
      if (ok) close();
      else install.disabled = false;
    });
  });
  const cancel = el("button", {
    class: "ghost",
    text: t("取消"),
    type: "button",
  });
  cancel.addEventListener("click", close);

  panel.append(
    el("p", {
      class: "hint",
      text: t(
        "粘贴一份 SKILL.md，或填一个网址。名字必须是小写 slug（字母、数字、连字符）—— 它会变成目录名；重名拒绝安装。",
      ),
    }),
    text,
    url,
    el("div", { class: "row" }, install, cancel),
  );
  text.focus();
}

/**
 * Add an MCP connector: one name plus **exactly one** transport (a remote url or a local
 * command — the server refuses both). `auth_env` holds the name of an environment
 * variable, never the key itself: mcp.json gets pasted into bug reports.
 */
async function mcpAddOverlay(): Promise<void> {
  const panel = el("div", {
    class: "fspanel cap-form",
    role: "dialog",
    "aria-label": t("添加 MCP 连接器"),
  });
  const close = openOverlay(panel);

  const name = el("input", {
    placeholder: t("名字，如 fs / notes"),
    "aria-label": t("名称"),
  });
  const url = el("input", {
    type: "url",
    placeholder: t("https://host/mcp（远程 HTTP 服务器）"),
    "aria-label": t("远程 URL"),
  });
  const command = el("input", {
    placeholder: t("npx（本地 stdio 服务器）"),
    "aria-label": t("本地命令"),
  });
  const args = el("input", {
    placeholder: t(
      "参数，空格分隔，如 -y @modelcontextprotocol/server-filesystem /tmp",
    ),
    "aria-label": t("命令参数"),
  });
  const authEnv = el("input", {
    placeholder: t("鉴权环境变量名（可留空），如 NOTES_API_KEY"),
    "aria-label": t("鉴权环境变量名"),
  });

  const save = el("button", {
    class: "approve",
    text: t("保存"),
    type: "button",
  });
  save.addEventListener("click", () => {
    const spec: McpServerSpec = { name: name.value.trim() };
    if (url.value.trim() !== "") {
      spec.url = url.value.trim();
    } else {
      spec.command = command.value.trim();
      const parts = args.value
        .trim()
        .split(/\s+/)
        .filter((part) => part !== "");
      if (parts.length > 0) spec.args = parts;
    }
    if (authEnv.value.trim() !== "") spec.auth_env = authEnv.value.trim();
    save.disabled = true;
    void capabilityWrite(() => saveMcpServer(spec), dom.mcpBody).then((ok) => {
      if (ok) close();
      else save.disabled = false;
    });
  });
  const cancel = el("button", {
    class: "ghost",
    text: t("取消"),
    type: "button",
  });
  cancel.addEventListener("click", close);

  panel.append(
    el("p", {
      class: "hint",
      text: t(
        "url 和 command 只能填一个。参数用空格分隔（路径里有空格就先编辑 mcp.json）。鉴权填环境变量的名字，不是密钥本身。保存后重启 Jeeves 生效 —— 连接发生在启动时。",
      ),
    }),
    el("label", { class: "cap-field" }, el("span", { text: t("名称") }), name),
    el(
      "label",
      { class: "cap-field" },
      el("span", { text: t("远程 URL") }),
      url,
    ),
    el(
      "label",
      { class: "cap-field" },
      el("span", { text: t("本地命令") }),
      command,
    ),
    el(
      "label",
      { class: "cap-field" },
      el("span", { text: t("命令参数") }),
      args,
    ),
    el(
      "label",
      { class: "cap-field" },
      el("span", { text: t("鉴权环境变量名") }),
      authEnv,
    ),
    el("div", { class: "row" }, save, cancel),
  );
  name.focus();
}

/**
 * Add a scheduled job: when it fires (cron) and what it does (one prompt).
 *
 * This form saves one job in the state database and nothing else — the Runtime fires it.
 * The panel's closing note says that out loud; a form that quietly needed a resident process
 * would be the kind of UI that looks finished and does nothing.
 */
async function scheduleAddOverlay(): Promise<void> {
  const panel = el("div", {
    class: "fspanel cap-form",
    role: "dialog",
    "aria-label": t("添加定时任务"),
  });
  const close = openOverlay(panel);

  const name = el("input", {
    placeholder: t("morning-news（小写字母、数字、连字符）"),
    "aria-label": t("名称"),
  });
  const cron = el("input", {
    class: "sched-cron",
    placeholder: "0 8 * * 1-5",
    "aria-label": t("cron 表达式"),
  });
  const runAt = el("input", {
    type: "datetime-local",
    "aria-label": t("提醒时间"),
  });
  runAt.hidden = true;
  const kind = el(
    "select",
    { "aria-label": t("重复还是一次") },
    el("option", { value: "cron", text: t("重复（cron）") }),
    el("option", { value: "once", text: t("只一次") }),
  );
  kind.addEventListener("change", () => {
    const once = kind.value === "once";
    cron.hidden = once;
    runAt.hidden = !once;
  });
  const prompt = el("textarea", {
    class: "cap-textarea",
    rows: 4,
    placeholder: t(
      "到点让 Jeeves 做的事，比如：给我一份今天的科技简报，中文，三条以内",
    ),
    "aria-label": t("触发时做什么"),
  });
  const delivery = el(
    "select",
    { "aria-label": t("投递方式") },
    el("option", { value: "outbox", text: t("outbox —— 写进 <home>/outbox/") }),
    el("option", {
      value: "notify",
      text: t("notify —— 写文件，同时推送通知"),
    }),
    el("option", { value: "quiet", text: t("quiet —— 只记日志（高频任务）") }),
  );

  const save = el("button", {
    class: "approve",
    text: t("保存"),
    type: "button",
  });
  save.addEventListener("click", () => {
    const once = kind.value === "once";
    if (once && runAt.value === "") {
      runAt.focus();
      return;
    }
    const spec: ScheduleSpec = {
      name: name.value.trim(),
      cron: once ? "" : cron.value.trim(),
      prompt: prompt.value.trim(),
      delivery: delivery.value,
      ...(once ? { runAt: runAt.value.replace("T", " ") } : {}),
    };
    save.disabled = true;
    void capabilityWrite(() => saveSchedule(spec), dom.schedulesBody).then(
      (ok) => {
        if (ok) close();
        else save.disabled = false;
      },
    );
  });
  const cancel = el("button", {
    class: "ghost",
    text: t("取消"),
    type: "button",
  });
  cancel.addEventListener("click", close);

  panel.append(
    el("p", {
      class: "hint",
      text: t(
        "cron 五段：分 时 日 月 周。`0 8 * * 1-5` 是工作日八点，`30 19 * * *` 是每天十九点半。触发时会在自己的会话里跑一轮。",
      ),
    }),
    el("p", {
      class: "hint",
      text: t("选「只一次」就填一个本地时间：到点跑一轮，跑完自动删除。"),
    }),
    el("label", { class: "cap-field" }, el("span", { text: t("名称") }), name),
    el("label", { class: "cap-field" }, el("span", { text: t("方式") }), kind),
    el(
      "label",
      { class: "cap-field" },
      el("span", { text: t("时间") }),
      cron,
      runAt,
    ),
    el(
      "label",
      { class: "cap-field" },
      el("span", { text: t("做什么") }),
      prompt,
    ),
    el(
      "label",
      { class: "cap-field" },
      el("span", { text: t("投递") }),
      delivery,
    ),
    el("div", { class: "row" }, save, cancel),
  );
  name.focus();
}



function removeScheduleAsk(job: ScheduleJob): void {
  const ok = window.confirm(
    t(
      "删除定时任务「{name}」？这一条会从状态库移掉，下次触发前 Runtime 就读不到它。已经写出的 outbox 文件不动。",
      { name: job.name },
    ),
  );
  if (!ok) return;
  void capabilityWrite(() => removeSchedule(job.name), dom.schedulesBody);
}

async function submit(): Promise<void> {
  if (preparing) return;
  const text = dom.input.value.trim();
  if (text === "" && staged.length === 0) return;
  webState.view = "chat";

  if (busy || otherTurn !== null) {
    if (staged.length > 0) {
      showStatus("thinking", t("这一轮还在跑 —— 附件留到下一轮"));
      return;
    }
    if (text === "") return;
    if (queuedMode) await followUp(text);
    else await steer(text);
    return;
  }

  let full = text;
  let attachments: TurnAttachment[] | null = null;
  if (staged.length > 0) {
    const home = jeevesHome;
    const lines: string[] = [];
    try {
      for (const file of staged) {
        const path = await uploadFile(file);
        if (file.type.startsWith("image/")) {
          if (file.size > 20 * 1024 * 1024) {
            dom.staged.replaceChildren(
              failureOf(
                new Error(t("{name} 超过 20MB 单图上限", { name: file.name })),
              ),
            );
            return;
          }
          attachments ??= [];
          attachments.push({ path, mediaType: file.type });
        } else {
          lines.push(t("（附件：{home}/{path}）", { home: home, path: path }));
        }
      }
    } catch (error: unknown) {
      dom.staged.replaceChildren(failureOf(error));
      return;
    }
    if (lines.length > 0) {
      full = `${lines.join("\n")}${text === "" ? "" : `\n${text}`}`;
    }
  }

  const snapshot = staged.slice();
  sessionStorage.removeItem(accountKey("jeeves.chat-draft." + current));
  dom.input.value = "";
  dom.input.style.height = "auto";
  staged.length = 0;
  renderStaged();
  await send(full, attachments, snapshot);
}

/**
 * Add a sentence to the turn that is already running (0.16).
 *
 * The bubble is painted right away because the message **is** accepted — it lands in the
 * transcript either way. Whether the model saw it before the turn ended is not something this
 * page can promise, so it does not claim it: "applies from the next turn" is the honest wording.
 */
async function steer(text: string): Promise<void> {
  sessionStorage.removeItem(accountKey("jeeves.chat-draft." + current));
  dom.input.value = "";
  dom.input.style.height = "auto";
  dom.messages.append(
    el("div", { class: "msg me" }, el("div", { class: "bubble", text })),
  );
  scrollToEnd(true);

  try {
    const { queued } = await steerTurn({ sessionId: current, message: text });
    showStatus(
      "thinking",
      queued > 1
        ? t("已补充（本轮第 {queued} 句，排队）", { queued: queued })
        : t("已补充，下一轮次生效"),
    );
  } catch (error: unknown) {
    dom.messages.append(failureOf(error));
    scrollToEnd(true);
  }
}

/**
 * Queue this sentence to run **after** the turn in flight (0.16).
 *
 * The person's wording is what makes this method exist: `steer` means "and also, while you are
 * at it", this means "when you are done". So it becomes its own turn with its own rounds and
 * budget, and it runs even if this page closes — the queue outlives the browser.
 *
 * The bubble is painted now because the sentence **is** accepted now; the statusline carries
 * the part that differs: it runs later, behind whatever is already waiting.
 */
async function followUp(text: string): Promise<void> {
  sessionStorage.removeItem(accountKey("jeeves.chat-draft." + current));
  dom.input.value = "";
  dom.input.style.height = "auto";
  dom.messages.append(
    el("div", { class: "msg me" }, el("div", { class: "bubble", text })),
  );
  scrollToEnd(true);

  try {
    const { queued } = await followUpTurn({
      sessionId: current,
      message: text,
    });
    showStatus(
      "thinking",
      queued > 1
        ? t("已排队 —— 前面还有 {n} 条", { n: queued - 1 })
        : t("已排队 —— 这一轮跑完就轮到它"),
    );
  } catch (error: unknown) {
    dom.messages.append(failureOf(error));
    scrollToEnd(true);
    void syncTurns();
  }
}

/** Say one thing, painting the course of this turn. */
async function send(
  text: string,
  attachments: TurnAttachment[] | null = null,
  previewFiles: File[] = [],
): Promise<void> {
  setBusy(true);

  dom.messages.querySelector(".welcome")?.remove();

  const mine = el(
    "div",
    { class: "msg me" },
    el("div", { class: "bubble", text }),
  );
  if (previewFiles.length > 0) {
    mine.append(
      attachmentStrip(
        previewFiles.map((file) => ({
          name: file.name,
          url: previewUrl(file),
        })),
      ),
    );
  }
  dom.messages.append(mine);

  const timeline = createTimeline();
  const trace = el("div", { class: "trace" });
  const bubble = el("div", { class: "bubble md streaming" });
  const stream = new StreamMarkdown(bubble);
  const foot = el("div", { class: "foot" });
  const cards = el("div", { class: "chat-task-slot" });
  dom.messages.append(
    el("div", { class: "msg jeeves" }, timeline.root, trace, bubble, cards, foot),
  );
  scrollToEnd(true);

  let streamed = "";
  let completed: TurnCompletedNotification | null = null;
  let failed = false;
  let firstDelta = true;
  showStatus("thinking", t("思考中"));

  try {
    const targetTask = webTasks.state.selected;
    const submission = await submissions.prepare({
      sessionId: current,
      message: text,
      taskId: targetTask || null,
      cwd: targetTask ? null : workspace,
      attachments,
    });
    const events = interact(submission);
    for await (const notification of events) {
      if (notification.type === "turnCompleted") {
        webTasks.state.selected = "";
        submissions.complete(submission);
      }
      switch (notification.type) {
        case "permissionChanged":
          if (notification.sessionId === current)
            webState.policy = notification;
          document
            .querySelectorAll<HTMLButtonElement>(".approval-card button")
            .forEach((button) => (button.disabled = true));
          break;
        case "turnStarted":
          activeTurnId = notification.turnId;
          addStopButton();
          break;

        case "textDelta":
          if (firstDelta) {
            firstDelta = false;
            showStatus("thinking", t("回复中"));
          }
          streamed += notification.delta;
          stream.push(notification.delta);
          scrollToEnd(true);
          break;

        case "approvalRequested": {
          const card = el("div", { class: "approval-card" });
          card.append(
            el(
              "div",
              { class: "ac-head" },
              el("span", { class: "ac-dot", "aria-hidden": "true" }),
              el("span", { class: "ac-title", text: t("需要你授权") }),
              el("span", { class: "tag", text: notification.tool }),
            ),
            el("code", { class: "ac-cmd", text: notification.argsPreview }),
          );
          const foot = el("div", { class: "ac-foot" });
          card.append(foot);
          const settle = (approved: boolean, remember: RememberScope) => {
            card.querySelectorAll("button").forEach((button) => {
              (button as HTMLButtonElement).disabled = true;
            });
            void respondApproval({
              turnId: notification.turnId,
              requestId: notification.requestId,
              approved,
              remember,
              version: notification.version,
            })
              .then((response) => {
                foot.replaceChildren(
                  chip(
                    response.accepted
                      ? approved
                        ? remember === "always"
                          ? t("已允许，并记住")
                          : remember === "session"
                            ? t("已允许，本会话不再问")
                            : t("已允许（只这次）")
                        : t("已拒绝")
                      : t("回答得太晚，那一轮已经不等了"),
                    "note",
                  ),
                );
              })
              .catch((error: unknown) => {
                foot.replaceChildren(
                  chip(
                    t("回答没送出去：{error}", { error: String(error) }),
                    "note",
                  ),
                );
              });
          };
          const allow = el("button", {
            class: "approve",
            text: t("只允许这次"),
          });
          const session = el("button", {
            class: "approve",
            text: t("本会话总是允许"),
            title: t("这次启动内不再问它（不写盘，重启就忘）"),
          });
          const always = el("button", {
            class: "approve",
            text: t("记住这条"),
            title: t("写进 settings.json，下次启动也不再问"),
          });
          const deny = el("button", { class: "deny", text: t("拒绝") });
          allow.addEventListener("click", () => settle(true, "once"));
          session.addEventListener("click", () => settle(true, "session"));
          always.addEventListener("click", () => settle(true, "always"));
          deny.addEventListener("click", () => settle(false, "once"));
          foot.append(deny, allow);
          if (webState.policy?.mode === null) foot.append(session, always);
          trace.append(card);
          break;
        }

        case "goalRound":
          timeline.goal(
            t("目标 · 第 {round}/{maxRounds} 轮 · {status} — {reason}", {
              round: notification.round,
              maxRounds: notification.maxRounds,
              status: notification.status,
              reason: notification.reason,
            }),
          );
          break;

        case "retry":
          timeline.retry(
            t("重试第 {attempt} 次（{reason}，{delay} 后）", {
              attempt: notification.attempt,
              reason: notification.reason,
              delay: formatMs(notification.delayMs),
            }),
          );
          break;

        case "toolStarted": {
          showStatus("working", notification.tool);
          timeline.toolStart(
            notification.toolUseId,
            notification.tool,
            toolSummary(notification),
          );
          scrollToEnd(true);
          break;
        }

        case "toolDelta": {
          timeline.toolDelta(notification.toolUseId, notification.chunk);
          scrollToEnd(false);
          break;
        }

        case "toolCompleted": {
          timeline.toolEnd(
            notification.toolUseId,
            notification.tool,
            notification.status === "ok",
            notification.durationMs,
            notification.output,
          );
          if (notification.tool === "task" && notification.status === "ok") {
            const started = taskCards(queuedTasks(notification.output), webTasks, openWorkFromReceipt);
            if (started) { cards.append(started); void webTasks.refresh(); }
          }
          scrollToEnd(true);
          break;
        }

        case "consolidationCompleted":
          if (notification.newFacts > 0) {
            timeline.note(
              t("记住了 {newFacts} 条新事实", {
                newFacts: notification.newFacts,
              }),
            );
            void refresh();
          }
          break;

        case "turnCompleted":
          completed = notification;
          break;

        case "error":
          trace.append(failure(notification));
          failed = true;
          break;
      }
    }
  } catch (error) {
    trace.append(failureOf(error));
    failed = true;
  } finally {
    bubble.classList.remove("streaming");
    hideStatus();
    timeline.settle();

    if (completed !== null) {
      stream.cancel();
      paintReply(
        bubble,
        completed.reply === "" ? t("（这一轮没有回复）") : completed.reply,
      );
      foot.textContent = turnFooter(completed);
    } else if (streamed !== "") {
      stream.finish();
      foot.textContent = t("这一轮没有收到结尾 —— 上面的回复可能被截断了");
    } else if (failed) {
      bubble.remove();
    } else {
      bubble.textContent = t("（这一轮没有回复）");
    }

    scrollToEnd(true);
    setBusy(false);
    void refresh();
    trimMessages();
    dom.input.focus();
  }
}

/**
 * Swap the streamed paint for the authoritative reply, exactly once.
 *
 * The stream already renders Markdown live (StreamMarkdown); this full repaint exists
 * only because `reply` from app-server is the truth — deltas may be missing
 * (backpressure, disconnection) or differ from what actually landed in the transcript.
 */
function paintReply(bubble: HTMLElement, markdown: string): void {
  bubble.classList.add("md");
  bubble.replaceChildren(renderMarkdown(markdown));
}

/** Open the server-owned main conversation and refresh its tasks. */
async function openMain(): Promise<void> {
  if (busy) return;
  try {
    const { sessionId } =
      await preferenceRequest<import("./protocol").SessionNewResponse>(
        "/api/session/main",
      );
    webState.mainSessionId = sessionId;
    webTasks.state.owner = sessionId;
    await refresh();
    await open(sessionId);
    await webTasks.refresh();
      dom.input.focus();
  } catch (error) {
    webState.message = String(error);
  }
}

async function open(sessionId: string): Promise<void> {
  if (sessionId !== webState.mainSessionId) return;
  if (busy) return;

  captureReading();
  rememberSession(sessionId);
  if (sessionId !== webState.mainSessionId) webTasks.state.selected = "";
  webState.sessionId = sessionId;
  queuedMode = false;
  dom.session.textContent = sessionId === webState.mainSessionId ? "Jeeves" : sessionId;
  dom.older.hidden = true;

  void fetchSessionMeta(sessionId)
    .then((meta) => {
      if (current !== sessionId) return;
      setWorkspace(meta.session.cwd ?? null);
    })
    .catch((error: unknown) => {
      console.warn(t("会话的元数据没读到（工作目录沿用当前）"), error);
    });

  try {
    await loadHistory(sessionId, null);
  } catch (error) {
    dom.messages.replaceChildren(failureOf(error));
    return;
  }
  restoreReading();
  void syncTurns();

  void refresh();
}

/**
 * Fetch one page of history and paint it.
 *
 * Empty `cursor` = switching sessions or first open, replace the whole screen; given = paging
 * back, prepended at the front. This is how `session/messages` works: the protocol gives
 * **newest first** (easy to page toward earlier), the screen needs it upright, so flip here.
 */
async function openWorkFromReceipt(id: string): Promise<boolean> {
  if (!await webTasks.openReceipt(id, "workspace")) return false;
  workspaceActions.openWork(id);
  return true;
}

/** A history row, with the live cards of any tasks that turn started placed under its reply. */
function historyRow(message: import("./protocol").Message): HTMLElement {
  const row = messageRow(message, openWorkFromReceipt);
  const started = message.meta?.startedTasks ?? [];
  const cards = taskCards(started.map(id => ({ id, title: "" })), webTasks, openWorkFromReceipt);
  if (cards) row.querySelector(".message-meta")?.before(cards);
  return row;
}

async function loadHistory(
  sessionId: string,
  cursor: string | null,
): Promise<void> {
  const page = await fetchMessages(sessionId, cursor);
  if (sessionId !== current) return;
  const nodes = [...page.data].reverse().map(historyRow);
  if (cursor === null) {
    historySession = sessionId;
    newestHistoryId = page.data[0]?.id ?? 0;
  }

  if (cursor === null) {
    dom.messages.replaceChildren(...(nodes.length === 0 ? [welcome(fillComposer)] : nodes));
    trimMessages();
  } else {
    const fromBottom = dom.messages.scrollHeight - dom.messages.scrollTop;
    dom.messages.prepend(...nodes);
    dom.messages.scrollTop = dom.messages.scrollHeight - fromBottom;
  }

  olderCursor = page.nextCursor;
  dom.older.hidden = olderCursor === null;
}

/** Background task rows can arrive while the main chat is idle. Page to the
 * last observed row so a burst of notices cannot fall between polls. */
async function appendTaskNotices(): Promise<void> {
  const session = current;
  if (busy || readingNotices || session !== webState.mainSessionId || session !== historySession) return;
  readingNotices = true;
  const from = newestHistoryId;
  try {
    const notices: import("./protocol").Message[] = [];
    let cursor: string | null = null;
    let latest = from;
    do {
      const page = await fetchMessages(session, cursor);
      latest = Math.max(latest, page.data[0]?.id ?? 0);
      for (const row of page.data) {
        if (row.id > from && (taskNotice(row) || row.source?.startsWith("responsibility:"))) notices.push(row);
      }
      cursor = (page.data.at(-1)?.id ?? 0) > from ? page.nextCursor : null;
    } while (cursor);
    if (busy || current !== session || historySession !== session || from !== newestHistoryId) return;
    const followingLatest = nearBottom();
    for (const row of notices.reverse()) {
      if (dom.messages.querySelector(`[data-notice-id="${row.id}"]`)) continue;
      dom.messages.querySelector(".welcome")?.remove();
      dom.messages.append(messageRow(row, openWorkFromReceipt));
    }
    newestHistoryId = latest;
    if (notices.length && followingLatest) pinToEnd();
  } catch (error) {
    console.warn(t("任务消息读取失败"), error);
  } finally { readingNotices = false; }
}

/** Page back one screen: older messages. */
async function older(): Promise<void> {
  if (busy || olderCursor === null) return;

  dom.older.disabled = true;
  try {
    await loadHistory(current, olderCursor);
  } catch (error) {
    dom.messages.prepend(failureOf(error));
  } finally {
    dom.older.disabled = false;
  }
}

/**
 * Ask `turn/list`: is anything running on this session right now?
 *
 * Skipped while this page streams its own turn — `busy` already answers, and a probe would
 * race with that turn's start and end. A failed probe keeps the previous answer: app-server
 * may be restarting, and flickering between "running" and "nothing" is worse than going
 * quiet for one poll.
 */
async function syncTurns(): Promise<void> {
  if (busy) return;
  const sessionId = current;
  try {
    const { turns } = await fetchTurns(sessionId);
    if (current !== sessionId) return;
    setOtherTurn(turns[0] ?? null);
  } catch {
  }
}

/**
 * Record that a turn runs elsewhere, and say so on the statusline — the one place this page
 * already talks about the current turn. Re-asserted on every poll so the elapsed time stays
 * current (and so a steer confirmation, which paints the same line, cannot outlive it).
 *
 * Stopping it from here is legitimate too: `turn/interrupt` works by id alone, so the stop
 * button gets that id — "it is stuck over there" is still the human's problem to solve.
 */
function setOtherTurn(turn: TurnView | null): void {
  const wasRunning = otherTurn !== null;
  otherTurn = turn?.startedAt ?? null;
  webState.busy = busy || otherTurn !== null;
  dom.composer.classList.toggle("elsewhere", turn !== null);
  paintComposerMode();

  if (turn === null) {
    if (wasRunning && !busy) hideStatus();
    return;
  }
  if (busy) return;
  const since = Date.parse(turn.startedAt);
  showStatus(
    "working",
    Number.isNaN(since)
      ? t("这一轮在别处跑 —— 可以补充它，也可以跑完再说")
      : t("这一轮在别处跑了 {elapsed} —— 可以补充它，也可以跑完再说", {
          elapsed: elapsedSince(since),
        }),
  );
  activeTurnId = turn.turnId;
  addStopButton();
}

/**
 * Say what pressing send will actually do right now.
 *
 * One button, three meanings — a fresh turn (idle), adding to the turn in flight (steer), or
 * queueing behind it (follow-up) — so the label changes with the moment instead of leaving the
 * person to guess. The toggle itself only exists while there is a turn to follow.
 */
function paintComposerMode(): void {
  const inFlight = busy || otherTurn !== null;
  const queuedNow = inFlight && queuedMode;
  dom.queueToggle.hidden = !inFlight;
  dom.queueToggle.classList.toggle("on", queuedNow);
  dom.queueToggle.setAttribute("aria-pressed", String(queuedNow));
  dom.send.textContent = !inFlight
    ? t("发送")
    : queuedNow
      ? t("排队")
      : t("补充");
}

function setBusy(value: boolean): void {
  busy = value;
  webState.busy = busy || otherTurn !== null;
  if (value) {
    queuedMode = false;
    setOtherTurn(null);
  }
  dom.send.disabled = value;
  dom.composer.classList.toggle("busy", value);
  paintComposerMode();
}

/**
 * Scrolling policy: follow the stream **only while the reader is at the bottom** — someone
 * paging back through history must not be yanked down by every delta. `interactive` marks
 * a moment that always scrolls (the user just sent something).
 */
function nearBottom(): boolean {
  return (
    dom.messages.scrollHeight -
      dom.messages.scrollTop -
      dom.messages.clientHeight <
    120
  );
}

/**
 * Show the "jump to latest" button whenever the newest message is out of view — someone
 * scrolled up to read history while the chat kept moving. Hidden the moment they are back
 * near the bottom, so it never covers the composer during normal reading.
 */
function updateJumpLatest(): void {
  dom.jumpLatest.hidden = nearBottom();
}

/** Remember the current session's distance from the bottom, before leaving it. */
function captureReading(): void {
  const m = dom.messages;
  reading.set(
    current,
    m.scrollHeight > m.clientHeight ? m.scrollHeight - m.scrollTop : 0,
  );
}

/**
 * Put the view back where this session was left.
 *
 * Never opened in this sitting → the newest message (what "open this conversation" means).
 * Was at the bottom → stay at the bottom, through the late layout pass: people who read in
 * a chat expect the newest line, not a frozen gap.
 */
function restoreReading(): void {
  const fromBottom = reading.get(current);
  if (fromBottom === undefined || fromBottom <= 8) {
    pinToEnd();
    return;
  }
  placeAt(Math.max(0, dom.messages.scrollHeight - fromBottom));
}

function scrollToEnd(interactive = false): void {
  if (interactive || nearBottom()) pinToEnd();
  updateJumpLatest();
}

/**
 * Put the newest message in view, and keep it there across one more layout pass.
 *
 * `scrollTop = scrollHeight` lands at the bottom of what is laid out **right now**; an image
 * or a Markdown block that sizes itself a frame later pushes the last message back down by
 * its own height. Re-pin on the next frame — but only if nobody scrolled in between, or an
 * upload finishing would move the page under someone who is reading further up.
 */
function pinToEnd(): void {
  placeAt(dom.messages.scrollHeight);
  const pinned = dom.messages.scrollTop;
  requestAnimationFrame(() => {
    if (Math.abs(dom.messages.scrollTop - pinned) < 4) {
      placeAt(dom.messages.scrollHeight);
    }
  });
}

/**
 * Place the view **without** the stylesheet's smooth-scroll animation.
 *
 * `.messages` sets `scroll-behavior: smooth` so a jump of a few lines feels continuous. A
 * programmatic placement must not inherit it: opening a long conversation would slide from
 * the first message down to where you were — the exact flicker this code exists to avoid —
 * and per-delta streaming would spend the turn chasing an animation it never catches.
 */
function placeAt(top: number): void {
  const style = dom.messages.style;
  const previous = style.scrollBehavior;
  style.scrollBehavior = "auto";
  dom.messages.scrollTop = top;
  style.scrollBehavior = previous;
}

/**
 * Cap the live DOM: the store keeps everything, the page keeps a window. Beyond
 * MAX_MESSAGES children the oldest nodes are dropped — paging back is exactly what
 * the "Older" button is for, so nothing is lost, just not painted.
 */
const MAX_MESSAGES = 300;

function trimMessages(): void {
  const overflow = dom.messages.children.length - MAX_MESSAGES;
  if (overflow <= 0) return;
  for (let i = 0; i < overflow; i++) {
    dom.messages.firstElementChild?.remove();
  }
  if (dom.messages.querySelector(".folded-note") === null) {
    dom.messages.prepend(
      el("p", {
        class: "hint folded-note",
        text: t("更早的消息已折叠 —— 用上面的「更早的」翻看"),
      }),
    );
  }
}

function must<T extends HTMLElement = HTMLElement>(id: string): T {
  const node = document.getElementById(id);
  if (node === null) {
    stalePage(id);
    throw new Error(t("页面里没有 #{id}", { id: id }));
  }
  return node as T;
}

/**
 * The page and the bundle are not the same build — and that used to mean a blank screen.
 *
 * `index.html` is network-first while `main.js` is cache-first, so a service-worker cache
 * can hand a *new* bundle an *old* page. The `must()` above then threw at module scope,
 * which aborts the whole bundle: no session list, no chat, no error — just an empty page
 * and one console line nobody opens. This is that failure given a face: what happened,
 * and the single click that fixes it (drop the stale caches, refetch).
 *
 * Styling is inline on purpose: style.css may be the stale half too.
 */
function stalePage(id: string): void {
  if (document.body === null) return;
  const reload = el("button", {
    type: "button",
    text: t("清掉旧缓存并重新加载"),
    style:
      "margin-top:18px;padding:9px 18px;border:1px solid currentColor;border-radius:9px;" +
      "background:transparent;color:inherit;cursor:pointer;font:inherit",
  });
  reload.addEventListener("click", () => {
    void (async () => {
      const scope = new URL("./", location.href).href;
      const registration = await navigator.serviceWorker?.getRegistration?.(scope);
      if (registration?.scope === scope) await registration.unregister();
      if (typeof caches !== "undefined") {
        const keys = await caches.keys();
        await Promise.all(keys.filter(key => key.startsWith(`jeeves:${scope}:`) || /^jeeves-v\d+$/.test(key)).map(key => caches.delete(key)));
      }
      location.reload();
    })();
  });
  document.body.replaceChildren(
    el(
      "div",
      {
        style:
          "max-width:560px;margin:16vh auto 0;padding:0 24px;color:var(--text,CanvasText);" +
          "font:15px/1.75 -apple-system,system-ui,'Segoe UI',sans-serif",
      },
      el("h1", {
        text: t("页面和脚本不是同一版"),
        style: "font-size:18px;margin:0 0 14px",
      }),
      el("p", {
        text: t(
          "这个页面缺了 #{id} —— 浏览器缓存了旧版脚本（service worker 缓存优先），新页面配上了旧 bundle。",
          { id: id },
        ),
        style: "margin:0 0 6px",
      }),
      el("p", {
        text: t(
          "点下面的按钮：会注销 service worker、清掉旧缓存，再取一份新版。",
        ),
        style: "margin:0;color:var(--text-2,CanvasText)",
      }),
      reload,
    ),
  );
}

workspaceActions.refresh = refresh;
workspaceActions.browse = () => void cwdPicker();
workspaceActions.mountPanel = mountPanel;
workspaceActions.reviewOperations = (task) => void switchTask(task, "operations");
workspaceActions.upload = () => dom.file.click();
workspaceActions.cwdChanged = setWorkspace;

if (
  "serviceWorker" in navigator &&
  (location.protocol === "https:" ||
    location.hostname === "localhost" ||
    location.hostname === "127.0.0.1")
) {
  navigator.serviceWorker.register("./sw.js").catch(() => undefined);
}

void boot();
