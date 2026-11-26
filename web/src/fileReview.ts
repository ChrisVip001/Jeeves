import type { TaskReviewFile } from "./protocol";

export type WorkFile = TaskReviewFile & { workspace?: boolean };

type DiffRow = {
  before?: number;
  after?: number;
  kind: "context" | "addition" | "deletion" | "hunk";
  text: string;
};

export function diffRows(patch: string): DiffRow[] {
  if (!patch) return [];
  let before = 0;
  let after = 0;
  return patch.replace(/\n$/, "").split("\n").map(line => {
    const hunk = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/.exec(line);
    if (hunk) {
      before = Number(hunk[1]);
      after = Number(hunk[2]);
      return {kind: "hunk", text: line};
    }
    if (line.startsWith("+")) return {kind: "addition", after: after++, text: line};
    if (line.startsWith("-")) return {kind: "deletion", before: before++, text: line};
    if (line.startsWith(" ")) return {kind: "context", before: before++, after: after++, text: line};
    return {kind: "hunk", text: line};
  });
}
