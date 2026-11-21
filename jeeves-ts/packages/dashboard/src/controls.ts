/** Browser adapter for shared controls; conversation/domain controllers never import Vue components. */
import { reactive } from "vue";
import type { LarkConfig, LarkStatus } from "./ui/types.ts";
import { ApiError, preferenceRequest, saveMcpServer, updateSession } from "./api.ts";
import { t } from "./i18n.ts";
import type {
  DashboardData,
  ModelInfo,
  ModelListResponse,
  ModelProfileView,
  ModelProfilesResponse,
  ModelSaveParams,
  ModelSaveResponse,
  ConfigWriteParams,
  ConfigWriteResponse,
  SettingsView,
  SessionPermissionResponse,
  PermissionMode,
  CapabilityListResponse,
  CapabilityProbeResponse,
  ConnectorSettingsResponse,
  ConnectorSettingsSaveParams,
  ConnectorOAuthStartResponse,
  ConnectorCatalogResponse,
  ConnectorToolsResponse,
  ConnectorToolsSetParams,
} from "./protocol";

export const webState = reactive<{
  sessionId: string;
  mainSessionId: string;
  cwd?: string;
  busy: boolean;
  connected: boolean;
  config?: SettingsView;
  profiles: ModelProfileView[];
  models: ModelInfo[];
  policy?: SessionPermissionResponse;
  policySaving: boolean;
  policyLoading: boolean;
  policyError: string;
  message: string;
  /** The selected shell view is presentation state, independent of running tasks. */
  view: "activity" | "chat" | "cowork";
}>({
  sessionId: "",
  mainSessionId: "",
  busy: true,
  connected: false,
  profiles: [],
  models: [],
  policySaving: false,
  policyLoading: false,
  policyError: "",
  message: "",
  view: "chat",
});

export const workspaceActions: {
  openWork: (id: string) => void;
  refresh: () => Promise<void>;
  browse: () => void;
  upload: () => void;
  cwdChanged: (cwd: string | null) => void;
  main: () => Promise<void>;
} = {
  openWork: () => {},
  refresh: async () => {},
  browse: () => {},
  upload: () => {},
  cwdChanged: () => {},
  main: async () => {},
};
export function updateControls(data: DashboardData, sessionId: string): void {
  webState.config = data.config;
  webState.connected = true;
  if (data.sessions.some((s) => s.id === sessionId))
    webState.sessionId = sessionId;
}
export async function refreshConnections(): Promise<void> {
  const [profiles, models] = await Promise.all([
    preferenceRequest<ModelProfilesResponse>("/api/model/profiles"),
    preferenceRequest<ModelListResponse>("/api/model/catalog"),
  ]);
  webState.profiles = profiles.profiles;
  webState.models = models.data;
}
export async function refreshPolicy(
  session = webState.sessionId,
): Promise<void> {
  if (!session) return;
  webState.policyLoading = true;
  try {
    const policy = await preferenceRequest<SessionPermissionResponse>(
      `/api/session/permission?sessionId=${encodeURIComponent(session)}`,
    );
    if (webState.sessionId === session) {
      webState.policy = policy;
      webState.policyError = "";
    }
  } catch (error) {
    if (webState.sessionId === session) {
      webState.policy = undefined;
      webState.policyError =
        error instanceof ApiError && error.status === 404
          ? t("当前 Jeeves 服务不支持权限管理，请重启更新后的服务，再重新读取。")
          : t("权限读取失败：{error}", { error: String(error) });
    }
  } finally {
    if (webState.sessionId === session) webState.policyLoading = false;
  }
}
export async function setPermission(
  mode: PermissionMode,
  confirmed: boolean,
): Promise<void> {
  const policy = webState.policy;
  if (!policy || webState.policySaving) return;
  webState.policySaving = true;
  try {
    const saved = await preferenceRequest<SessionPermissionResponse>(
      "/api/session/permission",
      {
        sessionId: policy.sessionId,
        mode,
        expectedRevision: policy.revision,
        confirmFullAccess: confirmed,
      },
    );
    if (webState.sessionId === saved.sessionId) webState.policy = saved;
    document
      .querySelectorAll<HTMLButtonElement>(".approval-card button")
      .forEach((button) => (button.disabled = true));
    webState.message = t("权限已更新，待审批操作已失效。");
  } catch (error) {
    webState.message = String(error);
    await refreshPolicy(policy.sessionId);
  } finally {
    webState.policySaving = false;
  }
}
export function remember(subject: string, content: string): Promise<unknown> {
  return preferenceRequest("/api/memory/remember", { subject, content });
}
export const settingsActions = {
  capabilityList: () => preferenceRequest<CapabilityListResponse>("/api/capability"),
  capabilityProbe: (id: string) => preferenceRequest<CapabilityProbeResponse>("/api/capability/probe", { id }),
  connectorSettings: () => preferenceRequest<ConnectorSettingsResponse>("/api/connector/settings"),
  saveConnectorSettings: (params: ConnectorSettingsSaveParams) => preferenceRequest<ConnectorSettingsResponse>("/api/connector/settings/save", params),
  startConnectorOAuth: (id: string) => preferenceRequest<ConnectorOAuthStartResponse>("/api/connector/oauth/start", { id }),
  connectorCatalog: () => preferenceRequest<ConnectorCatalogResponse>("/api/connector/catalog"),
  connectorTools: (id: string) => preferenceRequest<ConnectorToolsResponse>(`/api/connector/tools?id=${encodeURIComponent(id)}`),
  setConnectorTool: (params: ConnectorToolsSetParams) => preferenceRequest<ConnectorToolsResponse>("/api/connector/tools/set", params),
  saveMcpServer,
  vault: () => preferenceRequest<{ names: string[] }>("/api/vault"),
  saveSecret: (name: string, value: string) =>
    preferenceRequest<{ names: string[] }>("/api/vault", { name, value }),
  removeSecret: (name: string) =>
    preferenceRequest<{ names: string[] }>("/api/vault/remove", { name }),
  inbox: () =>
    preferenceRequest<{ items: import("./ui/types.ts").InboxItem[] }>(
      "/api/inbox",
    ),
  lark: () => preferenceRequest<LarkStatus>("/api/connections/lark"),
  saveLark: (config: LarkConfig, appSecret: string | null) =>
    preferenceRequest<LarkStatus>("/api/connections/lark", {
      config,
      appSecret,
    }),
  claimLark: (openId: string) =>
    preferenceRequest<LarkStatus>("/api/connections/lark/claim", { openId }),
  saveModel: (params: ModelSaveParams) =>
    preferenceRequest<ModelSaveResponse>("/api/model/save", params),
  saveConfig: (params: ConfigWriteParams) =>
    preferenceRequest<ConfigWriteResponse>("/api/config", params),
};
export async function activateModel(id: string): Promise<void> {
  try {
    await settingsActions.saveModel({
      id,
      makeActive: true,
      provider: null,
      model: null,
      smallModel: null,
      baseUrl: null,
      apiKey: null,
      keyEnv: null,
      vision: null,
    });
    await refreshConnections();
    await workspaceActions.refresh();
  } catch (error) {
    webState.message = String(error);
  }
}
export async function saveWorkspace(
  cwd: string,
  sessionId = webState.sessionId,
): Promise<boolean> {
  if (!sessionId) return false;
  try {
    const response = await updateSession({ sessionId, cwd });
    if (webState.sessionId === sessionId)
      workspaceActions.cwdChanged(response.session.cwd ?? null);
    await workspaceActions.refresh();
    return true;
  } catch (error) {
    webState.message = String(error);
    return false;
  }
}
