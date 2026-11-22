import { accountKey } from "./hosted.ts";
import { preferenceRequest } from "./api.ts";
import type {
  ApprovalNoticeParams,
  DeviceListResponse,
  DeviceGrantView,
  ApprovalPushKeyResponse,
  ApprovalPushSubscribeParams,
} from "./protocol.ts";

function base64(bytes: ArrayBuffer): string {
  return btoa(String.fromCharCode(...new Uint8Array(bytes)));
}
function keyBytes(url64: string): Uint8Array<ArrayBuffer> {
  return Uint8Array.from(
    atob(url64.replace(/-/g, "+").replace(/_/g, "/")),
    (c) => c.charCodeAt(0),
  );
}
async function identity(create = true): Promise<CryptoKeyPair> {
  if (
    !globalThis.crypto?.subtle ||
    !globalThis.indexedDB ||
    !navigator.serviceWorker ||
    !globalThis.PushManager
  )
    throw Error("unsupported");
  const db = await new Promise<IDBDatabase>((resolve, reject) => {
    const req = indexedDB.open(accountKey("jeeves-approval-device"), 1);
    req.onupgradeneeded = () => req.result.createObjectStore("identity");
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(Error("failed"));
  });
  try {
    const read = () =>
      new Promise<CryptoKeyPair | undefined>((resolve, reject) => {
        const req = db
          .transaction("identity")
          .objectStore("identity")
          .get("device");
        req.onsuccess = () => resolve(req.result);
        req.onerror = () => reject(Error("failed"));
      });
    const existing = await read();
    if (existing) return existing;
    if (!create) throw Error("revoked");
    const pair = (await crypto.subtle
      .generateKey("Ed25519", false, ["sign", "verify"])
      .catch(() => {
        throw Error("unsupported");
      })) as CryptoKeyPair;
    const stored = await new Promise<boolean>((resolve, reject) => {
      const tx = db.transaction("identity", "readwrite");
      tx.objectStore("identity").add(pair, "device");
      tx.oncomplete = () => resolve(true);
      tx.onabort = () =>
        tx.error?.name === "ConstraintError"
          ? resolve(false)
          : reject(Error("failed"));
    });
    const winner = stored ? pair : await read();
    if (!winner) throw Error("failed");
    return winner;
  } finally {
    db.close();
  }
}

export async function prepareApprovalPush(): Promise<string> {
  const pair = await identity();
  return base64(await crypto.subtle.exportKey("raw", pair.publicKey));
}

let ready:
  | {
      pair: CryptoKeyPair;
      grant: DeviceGrantView;
      key: Uint8Array<ArrayBuffer>;
      registration: ServiceWorkerRegistration;
    }
  | undefined;
export async function checkApprovalPush(): Promise<void> {
  ready = undefined;
  const pair = await identity();
  const id = base64(await crypto.subtle.exportKey("raw", pair.publicKey));
  const list = await preferenceRequest<DeviceListResponse>("/api/device");
  const grant = list.devices.find(
    (row) =>
      row.deviceId === id &&
      !row.revoked &&
      row.capabilities.includes("approvalReview"),
  );
  if (!grant) throw Error("unpaired");
  const key = await preferenceRequest<ApprovalPushKeyResponse>(
    "/api/approval/push-key",
    {},
  );
  if (!key.publicKey) throw Error("unconfigured");
  const registration = await navigator.serviceWorker.getRegistration();
  if (!registration?.active) throw Error("unsupported");
  ready = { pair, grant, registration, key: keyBytes(key.publicKey) };
}
export async function enableApprovalPush(): Promise<void> {
  const context = ready;
  if (!context) throw Error("unpaired");
  const sub = await context.registration.pushManager
    .subscribe({ userVisibleOnly: true, applicationServerKey: context.key })
    .catch((err: unknown) => {
      throw Error(
        err instanceof DOMException && err.name === "NotAllowedError"
          ? "denied"
          : "failed",
      );
    });
  const keys = sub.toJSON().keys;
  if (!keys?.p256dh || !keys.auth) throw Error("failed");
  const { deviceId, grantId, generation, lastSequence } = context.grant;
  const sequence = lastSequence + 1;
  const fields = [
    "jeeves.approval.push.v1",
    deviceId,
    grantId,
    generation,
    sequence,
    sub.endpoint,
    keys.p256dh,
    keys.auth,
  ];
  const signature = base64(
    await crypto.subtle.sign(
      "Ed25519",
      context.pair.privateKey,
      new TextEncoder().encode(JSON.stringify(fields)),
    ),
  );
  const params: ApprovalPushSubscribeParams = {
    deviceId,
    grantId,
    generation,
    sequence,
    endpoint: sub.endpoint,
    p256dh: keys.p256dh,
    auth: keys.auth,
    signature,
  };
  await preferenceRequest("/api/approval/push-subscribe", params);
  ready = undefined;
}

export async function signApprovalNotice(
  notice: ApprovalNoticeParams,
  approved: boolean | null,
): Promise<string> {
  const pair = await identity(false);
  if (
    base64(await crypto.subtle.exportKey("raw", pair.publicKey)) !==
    notice.deviceId
  )
    throw Error("revoked");
  const fields = [
    "jeeves.approval.notice.v1",
    notice.deviceId,
    notice.grantId,
    notice.generation,
    notice.taskId,
    notice.turnId,
    notice.approvalRequestId,
    notice.approvalVersion,
    approved,
  ];
  return base64(
    await crypto.subtle.sign(
      "Ed25519",
      pair.privateKey,
      new TextEncoder().encode(JSON.stringify(fields)),
    ),
  );
}
