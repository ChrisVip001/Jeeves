import type { InteractionStartParams } from "../../../../../jeeves-rs/jeeves-protocol/schema/typescript/v2/InteractionStartParams";
type Store = Pick<Storage, "getItem" | "setItem" | "removeItem">;
/** Keep a message identity across ambiguous failures/reloads without storing its text. */
export function submissionTracker(storage: Store) {
  return {
    async prepare(
      input: Omit<InteractionStartParams, "messageId">,
    ): Promise<InteractionStartParams> {
      let left = 2166136261,
        right = 3335557771;
      for (const byte of new TextEncoder().encode(JSON.stringify(input))) {
        left = Math.imul(left ^ byte, 16777619);
        right = Math.imul(right ^ byte, 2246822519);
      }
      const digest = `${left >>> 0}:${right >>> 0}`;
      const key = `jeeves.pending-interaction.${input.sessionId}`;
      let previous: { digest: string; id: string } | undefined;
      try {
        previous = JSON.parse(storage.getItem(key) || "null");
      } catch {
      }
      const id =
        previous?.digest === digest
          ? previous.id
          : Array.from(crypto.getRandomValues(new Uint8Array(16)), (byte) =>
              byte.toString(16).padStart(2, "0"),
            ).join("");
      storage.setItem(key, JSON.stringify({ digest, id }));
      return { ...input, messageId: id };
    },
    complete(input: InteractionStartParams) {
      const key = `jeeves.pending-interaction.${input.sessionId}`;
      try {
        if (JSON.parse(storage.getItem(key) || "null")?.id === input.messageId)
          storage.removeItem(key);
      } catch {
      }
    },
  };
}
