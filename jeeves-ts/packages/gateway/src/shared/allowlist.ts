/**
 * Who may talk to the bot. One definition for every channel.
 *
 * Empty = answer everyone (the operator chose not to restrict). `claim` mode — no owner yet —
 * answers nobody and lets the adapter report the first sender as a candidate; it only applies
 * while the list is empty.
 */
export class Allowlist {
  readonly #ids: ReadonlySet<string>;
  readonly claiming: boolean;

  constructor(ids: readonly string[] = [], claim = false) {
    this.#ids = new Set(ids.map((id) => id.trim()));
    this.claiming = claim && this.#ids.size === 0;
  }

  /** `true` when the list restricts anyone at all. */
  get restricted(): boolean {
    return this.#ids.size > 0;
  }

  allows(id: string): boolean {
    return this.#ids.size === 0 || this.#ids.has(id);
  }
}
