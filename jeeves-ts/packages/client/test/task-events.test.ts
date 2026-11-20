import assert from "node:assert/strict";
import test from "node:test";

import { TaskEventFollower } from "../src/task-events.ts";
import type { TaskEventsParams, TaskEventsResponse, TaskEventView } from "../src/protocol.ts";

const ev = (cursor: number): TaskEventView => ({
  cursor,
  taskId: "t",
  runId: null,
  seq: cursor,
  kind: "k",
  detail: null,
  at: "x",
});

/** A fake server holding events 1..n, paging by `limit`. */
function server(total: () => number, pageSize = 2) {
  const calls: TaskEventsParams[] = [];
  const fetch = async (p: TaskEventsParams): Promise<TaskEventsResponse> => {
    calls.push(p);
    const from = p.cursor ?? 0;
    const all = Array.from({ length: total() }, (_, i) => ev(i + 1)).filter((e) => e.cursor > from);
    const events = all.slice(0, pageSize);
    return {
      events,
      nextCursor: events.at(-1)?.cursor ?? from,
      hasMore: all.length > pageSize,
      latestCursor: total(),
    };
  };
  return { fetch, calls };
}

test("a reconnect pulls only what is newer, across pages, without gaps", async () => {
  let n = 5;
  const s = server(() => n);
  const f = new TaskEventFollower(s.fetch);
  assert.deepEqual((await f.catchUp()).events.map((e) => e.cursor), [1, 2, 3, 4, 5]);
  n = 7;
  const next = await f.catchUp();
  assert.deepEqual(next.events.map((e) => e.cursor), [6, 7]);
  assert.equal(next.reset, false);
  assert.equal(s.calls.at(-1)?.cursor, 5);
});

test("repeated knocks add nothing", async () => {
  const s = server(() => 3);
  const f = new TaskEventFollower(s.fetch);
  await f.catchUp();
  assert.deepEqual((await f.catchUp()).events, []);
  assert.equal(f.cursor, 3);
});

test("a server whose log is behind our cursor forces a reset and reload", async () => {
  let n = 4;
  const s = server(() => n);
  const f = new TaskEventFollower(s.fetch);
  await f.catchUp();
  n = 2;
  const out = await f.catchUp();
  assert.equal(out.reset, true);
  assert.deepEqual(out.events.map((e) => e.cursor), [1, 2]);
});
