export { JeevesClient, JeevesError } from "./client.ts";
export type { JeevesClientOptions, MethodName, Methods } from "./client.ts";

export { APP_SERVER_COMMAND, StdioTransport, findJeevesBinary } from "./transport.ts";
export { HttpTransport } from "./http-transport.ts";
export type { StdioTransportOptions, Transport } from "./transport.ts";

export { CODES, METHODS } from "./protocol.ts";
export type * from "./protocol.ts";

export { TaskEventFollower } from "../../../../protocol/typescript/task-events.mts";
export type { CatchUp, FetchTaskEvents } from "../../../../protocol/typescript/task-events.mts";
