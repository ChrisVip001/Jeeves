import type { Cluster } from "./cluster.ts";
import type { Accounts } from "./accounts.ts";

/** A private, already running Runtime. Provisioning and sleep belong to its operator. */
export function singleRuntime(
  accounts: Accounts,
  email: string,
  password: string | undefined,
) {
  const existing = accounts.all();
  if (
    existing.length > 1 ||
    (existing[0] && existing[0].email !== email.trim().toLowerCase())
  ) {
    throw new Error(
      "single Runtime requires a dedicated account store for its configured owner",
    );
  }
  const address = email.trim().toLowerCase();
  const owner =
    existing[0] ??
    (password ? accounts.register(address, password) : undefined);
  if (owner) accounts.touch(owner.id, true);
  const cluster: Cluster = {
    async apply() {},
    async scale() {},
    async remove() {
      throw new Error("externally managed Runtime");
    },
  };
  return {
    get ownerId() {
      return accounts.byEmail(address)?.id ?? "";
    },
    registrationEmail: owner ? undefined : address,
    cluster,
  };
}
