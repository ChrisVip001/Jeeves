import type * as V2 from "../../../protocol/schema/typescript/v2/index";
export type { ModelInfo } from "../../../protocol/schema/typescript/v2/index";
export interface LarkConfig {
  enabled: boolean;
  appId: string;
  allow: string[];
  dispatchers: string[];
  domain: string;
}
export interface LarkStatus {
  config: LarkConfig;
  hasSecret: boolean;
  state: "stopped" | "running" | "exited";
  candidate: string | null;
  recent: string[];
}
export interface InboxItem {
  kind: "task" | "draft";
  id: string;
  title: string;
  preview: string;
  at: string;
}
export interface SettingsActions {
  capabilityList(): Promise<V2.CapabilityListResponse>;
  capabilityProbe(id: string): Promise<V2.CapabilityProbeResponse>;
  connectorSettings(): Promise<V2.ConnectorSettingsResponse>;
  saveConnectorSettings(params: V2.ConnectorSettingsSaveParams): Promise<V2.ConnectorSettingsResponse>;
  startConnectorOAuth(id: string): Promise<V2.ConnectorOAuthStartResponse>;
  connectorCatalog(): Promise<V2.ConnectorCatalogResponse>;
  connectorTools(id: string): Promise<V2.ConnectorToolsResponse>;
  setConnectorTool(params: V2.ConnectorToolsSetParams): Promise<V2.ConnectorToolsResponse>;
  saveMcpServer(spec: import("../api").McpServerSpec): Promise<import("../api").Outcome>;
  vault(): Promise<{ names: string[] }>;
  saveSecret(name: string, value: string): Promise<{ names: string[] }>;
  removeSecret(name: string): Promise<{ names: string[] }>;
  inbox(): Promise<{ items: InboxItem[] }>;
  lark(): Promise<LarkStatus>;
  saveLark(config: LarkConfig, appSecret: string | null): Promise<LarkStatus>;
  claimLark(openId: string): Promise<LarkStatus>;
  saveModel(params: V2.ModelSaveParams): Promise<V2.ModelSaveResponse>;
}

export type {
  AgentTaskView,
  TaskListResponse,
  SessionMessagesResponse,
  TaskArtifactsResponse,
  TaskEnvelope,
  ApprovalRespondParams,
  ApprovalRespondResponse,
  ApprovalRequestedNotification,
  ApprovalPendingResponse,
  ServerNotification,
  TaskEventView,
  TaskGitCheckpointParams,
  TaskGitCheckpointResponse,
  TaskGitCheckpointPolicyParams,
  TaskGitCheckpointPolicyResponse,
  CheckView,
  PermissionMode,
  SessionPermissionResponse,
} from "../../../protocol/schema/typescript/v2/index";
