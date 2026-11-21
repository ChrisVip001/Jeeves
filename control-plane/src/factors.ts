import { createHmac, randomBytes, timingSafeEqual } from "node:crypto";

const alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

export function newSecret() {
  return Array.from(randomBytes(32), (b) => alphabet[b & 31]).join("");
}

export function totp(secret: string, step: number) {
  const bits = Array.from(secret, (c) =>
    alphabet.indexOf(c).toString(2).padStart(5, "0"),
  ).join("");
  const key = Buffer.from(
    bits.match(/.{8}/g)!.map((byte) => parseInt(byte, 2)),
  );
  const counter = Buffer.alloc(8);
  counter.writeBigUInt64BE(BigInt(step));
  const digest = createHmac("sha1", key).update(counter).digest();
  const value = digest.readUInt32BE(digest[19]! & 15) & 0x7fffffff;
  return (value % 1_000_000).toString().padStart(6, "0");
}

export function acceptedStep(secret: string, input: string, last: number) {
  if (!/^\d{6}$/.test(input)) return;
  const now = Math.floor(Date.now() / 30_000);
  return [now, now - 1, now + 1].find(
    (step) =>
      step > last &&
      timingSafeEqual(Buffer.from(totp(secret, step)), Buffer.from(input)),
  );
}
