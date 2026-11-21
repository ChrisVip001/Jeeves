import { randomInt } from "node:crypto";

/** A conversation (= session) id is derived from the platform identity; no table to lose. */
export function conversationOf(channel: string, userId: string): string {
  return `${channel}:${userId}`;
}

const ALPHABET = "ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

/** A short code a person reads out or types to bind an identity (not a secret; single use). */
export function pairingCode(length = 6): string {
  let out = "";
  for (let i = 0; i < length; i++) out += ALPHABET[randomInt(ALPHABET.length)];
  return out;
}
