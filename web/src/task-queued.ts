/** One task the `task` tool queued, read from its `• <title> (<id>): queued` output lines. */
export interface QueuedTask { id: string; title: string }

const QUEUED = /^• (.+) \((task\d+-\d+)\): queued$/gm;

export function queuedTasks(output: string): QueuedTask[] {
  return [...output.matchAll(QUEUED)].map(match => ({ title: match[1] ?? "", id: match[2] ?? "" }));
}
