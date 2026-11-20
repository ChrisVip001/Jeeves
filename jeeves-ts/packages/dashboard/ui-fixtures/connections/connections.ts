import { createApp } from "vue";
import SettingsPanel from "../../src/ui/SettingsPanel.vue";
import { uiTranslatorKey } from "../../src/ui/translate.ts";
import { initLang, setLang, t } from "../../src/i18n.ts";
import type { CapabilityCard, CapabilityKind, CapabilityState, ConnectorSettingsResponse, ConnectorSettingsSaveParams } from "../../src/protocol.ts";
import type { ConnectorCatalogEntry, ConnectorToolsResponse, ConnectorToolsSetParams } from "../../src/protocol.ts";
import type { McpServerSpec } from "../../src/api.ts";
import "../../src/ui/style.css";
import "../../src/style.css";

const card = (id: string, title: string, kind: CapabilityKind, state: CapabilityState, extras: Partial<CapabilityCard> = {}): CapabilityCard => ({
  id, title, kind, state, identity: null, toolSource: kind,
  secretPointer: null, detail: null, lastCheckedAt: 1795050000, ...extras,
});
const cards: CapabilityCard[] = [
  card("model:local", "Local model", "model", "ready", { secretPointer: "env:FIXTURE_API_KEY", identity: "ollama / fixture" }),
  card("model:work", "Work model", "model", "unauthorized", { secretPointer: "keychain" }),
  card("mcp:calendar", "Calendar MCP", "mcp", "expired", { secretPointer: "file" }),
  card("mcp:mail", "Mail MCP", "mcp", "missingScope", { secretPointer: "file" }),
  card("connector:email", "Email", "connector", "unauthorized", { secretPointer: "vault" }),
  card("executor:local", "Local executor", "executor", "blockedByPolicy"),
  card("connector:feishu", "Feishu sender", "connector", "blockedByPolicy", { detail: "send allowlist is empty" }),
  card("channel:lark", "Feishu / Lark", "channel", "offline", { secretPointer: "memory" }),
  card("mcp:broken", "Broken config", "mcp", "unknown", { secretPointer: "FAKE-SECRET-DO-NOT-RENDER", detail: "FAKE-SECRET-DO-NOT-RENDER" }),
];
(cards[1] as unknown as Record<string, unknown>).apiKey = "FAKE-SECRET-DO-NOT-RENDER";
const lang = new URLSearchParams(location.search).get("lang") === "en" ? "en" : "zh";
if (initLang() !== lang) setLang(lang);
let settings: ConnectorSettingsResponse = {
  calendar: null,
  email: {
    username: "reader@example.test", from: "reader@example.test", credentialSource: "vault:MAIL_FIXTURE",
    imap: { host: "imap.example.test", port: 143, security: "plain" },
    smtp: { host: "smtp.example.test", port: 465, security: "tls" }, allowRecipients: [], live: false,
  },
};
if (new URLSearchParams(location.search).has("legacyPointer")) {
  settings = {
    calendar: { url: "https://calendar.example.test/fixture", username: "reader@example.test", credentialSource: null, timezone: "UTC" },
    email: { ...settings.email!, credentialSource: "" },
  };
}
const fixtureWrites: ConnectorSettingsSaveParams[] = [];
const fixtureAuth: string[] = [];
const fixtureMcp: McpServerSpec[] = [];
const fixtureToolChanges: ConnectorToolsSetParams[] = [];
const catalog: ConnectorCatalogEntry[] = [
  { id: "builtin", title: "Built-in example", setup: "builtin", endpoint: null, documentationUrl: "https://docs.example.test/builtin", manifest: { category: "mail", tools: {}, verbs: {} } },
  { id: "provider", title: "Provider example", setup: "providerSetup", endpoint: "https://mcp.example.test/provider", documentationUrl: "https://docs.example.test/provider", manifest: { category: "calendar", tools: {}, verbs: {} } },
  { id: "catalog-fixture", title: "Catalog example", setup: "bearer", endpoint: "https://mcp.example.test/fixture", documentationUrl: "https://docs.example.test/fixture", manifest: { category: "notes", tools: { read_note: { effect: "read", disabled: false } }, verbs: {} } },
];
const policies: Record<string, ConnectorToolsResponse> = {
  "mcp:calendar": { editable: true, tools: [{ name: "read_events", effect: "read", disabled: false }, { name: "send_event", effect: "send", disabled: true }] },
  "mcp:mail": { editable: false, tools: [{ name: "read_mail", effect: "read", disabled: false }] },
};
let failInboxOnce = new URLSearchParams(location.search).has("failInbox");
Object.assign(window, { fixtureWrites, fixtureAuth, fixtureMcp, fixtureToolChanges });
const app = createApp(SettingsPanel, {
  profiles: [], models: [], busy: false, initialTab: "connections",
  actions: {
    capabilityList: async () => ({ cards: structuredClone(cards) }),
    capabilityProbe: async (id: string) => ({ card: { ...cards.find(c => c.id === id)!, state: "ready" } }),
    connectorCatalog: async () => ({ entries: catalog }),
    connectorTools: async (id: string) => {
      if (!policies[id]) throw new Error("FAKE-SECRET-DO-NOT-RENDER");
      return structuredClone(policies[id]);
    },
    setConnectorTool: async (params: ConnectorToolsSetParams) => {
      fixtureToolChanges.push(structuredClone(params));
      const policy = policies[params.id]!;
      if (!policy.editable || params.tool === "send_event") throw new Error("FAKE-SECRET-DO-NOT-RENDER");
      policy.tools.find(tool => tool.name === params.tool)!.disabled = params.disabled;
      return structuredClone(policy);
    },
    saveMcpServer: async (spec: McpServerSpec) => {
      fixtureMcp.push(JSON.parse(JSON.stringify(spec)) as McpServerSpec);
      if (spec.name === "fail") throw new Error("FAKE-SECRET-DO-NOT-RENDER");
      cards.push(card(`mcp:${spec.name}`, spec.name, "mcp", "unknown"));
      return { ok: true, message: "FAKE-SECRET-DO-NOT-RENDER" };
    },
    connectorSettings: async () => {
      if (new URLSearchParams(location.search).has("failSettings")) throw new Error("FAKE-SECRET-DO-NOT-RENDER");
      return settings;
    },
    saveConnectorSettings: async (params: ConnectorSettingsSaveParams) => {
      fixtureWrites.push(structuredClone(params));
      if (params.calendar?.timezone === "Bad/Zone") throw new Error("FAKE-SECRET-DO-NOT-RENDER");
      settings = { calendar: params.calendar ?? settings.calendar, email: params.email ?? settings.email };
      return settings;
    },
    startConnectorOAuth: async (id: string) => {
      fixtureAuth.push(id);
      if (id === "mcp:mail") throw new Error("FAKE-SECRET-DO-NOT-RENDER");
      if (new URLSearchParams(location.search).has("badOAuth")) return { authorizationUrl: "javascript:FAKE-SECRET-DO-NOT-RENDER" };
      return { authorizationUrl: "https://auth.example.test/fixture?state=fixture" };
    },
    lark: async () => ({ config: { enabled: false, appId: "", allow: [], dispatchers: [], domain: "feishu" }, hasSecret: false, state: "stopped", candidate: null, recent: [] }),
    inbox: async () => {
      if (failInboxOnce) {
        failInboxOnce = false;
        throw new Error("Outbox read failed");
      }
      return { items: [] };
    },
    vault: async () => ({ names: ["MAIL_FIXTURE", "CALENDAR_FIXTURE"] }),
  },
});
app.provide(uiTranslatorKey, t);
app.mount("#app");
