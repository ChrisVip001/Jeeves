# ① Protocol and configuration: `jeeves-protocol` + `jeeves-config`

English | [简体中文](01-protocol-and-config.zh.md)

Two small crates sit under everything else. `jeeves-protocol` is the only definition of the contract between the engine and its clients. `jeeves-config` is the only place that knows which knobs exist. Reference tables live elsewhere: methods and notifications in [protocol.md](../protocol.md), every `JEEVES_*` variable in [configuration.md](../configuration.md).

## 1.1 Protocol: one definition, generated copies

The contract is Rust types in `jeeves-protocol/src/protocol/v2.rs` (plus `memory_center.rs`) and the JSON-RPC envelope in `rpc.rs`. The envelope follows the JSON-RPC spec and is kept apart from the business types so a field change never has to be checked against envelope compatibility.

TypeScript types, JSON Schema and the constant tables are generated from those types and committed under `jeeves-protocol/schema/`. CI regenerates and compares, so changing a type without regenerating fails the build.

### Wire conventions

- Suffixes: `*Params` (request), `*Response`, `*Notification` (push).
- Fields are camelCase on the wire.
- Integers are always `i32`. ts-rs maps `i64` to `bigint` while `JSON.parse` yields `number`; the mismatch compiles and fails at runtime. Storage uses `i64` and narrows at the boundary (`narrow()` in app-server clamps to `0..=i32::MAX`).
- `ServerNotification` is an internally tagged enum (`tag = "type"`). The transport wraps every variant in the JSON-RPC method `turn/notification`.

### Envelope details (`rpc.rs`)

- `RequestId` is untagged `Number(i32) | String`, so clients using either id style parse. It derives `Hash` because the server keys pending requests by it.
- `JsonRpcError.id` is optional: when the request cannot be parsed there is no id to echo.
- `codes` has the standard JSON-RPC range plus Jeeves's own: provider error, tool error, `NOT_IMPLEMENTED`, `SESSION_BUSY`. The last two stay separate from `METHOD_NOT_FOUND` because "not built", "occupied" and "misspelled" call for different client reactions. `SESSION_BUSY` carries the occupying turn's id.

## 1.2 The generation pipeline

```
v2.rs types ──(cargo test, ignored test)──> TS + JSON Schema + constants
                                         ──(prettier)──> committed schema/
```

- `export.rs` holds `protocol_types!`, the single list of exported types. The barrel `typescript/v2/index.ts`, the flattened `json/v2.json` and the schema are all driven from it. Forgetting a type there is silent: it simply does not appear. Add every new type to that list.
- Method names and error codes are values, not types, so `export.rs` reads them from `methods` and `codes` and writes `typescript/v2/methods.mts`, `codes.mts` and `json/consts.json`. They are `.mts` because they export runtime values; `ts-rs` output is type-only and stays `.ts`. Do not add a `package.json` with `"type": "module"` to that directory: it would reclassify every generated file.
- `json/v2.json` is draft-07 with all definitions at the top level, so consumers get flat models.

### Why the derives are no-ops outside tests

`lib.rs` swaps `TS` / `JsonSchema` between the real derives (`cfg(test)`) and empty ones from `jeeves-protocol-noop-macros` (everything else). The release build therefore depends on neither `ts-rs` nor `schemars`. The no-op crate must declare the same helper attributes (`ts`, `schemars`, `serde`) or `#[ts(...)]` on a type becomes an unknown-attribute error in non-test builds.

### Driver: `scripts/write_schema.py`

1. Runs the ignored test `export_tests::write_schema_fixtures` into a scratch directory.
2. Formats the TypeScript with prettier. This must precede any comparison: the committed files are formatted, the raw output is not.
3. In write mode it syncs the scratch output into `schema/` and deletes orphans (ts-rs never deletes). With `--check` it only compares and exits non-zero on drift. Without a usable prettier the TypeScript comparison is skipped with a warning; JSON is still compared.

`just write-app-server-schema` writes, `just check-app-server-schema` checks; `just check` includes the check.

### Adding a method

1. Add the constant to `methods` and the `*Params` / `*Response` types in `v2.rs`.
2. Add the new type names to `protocol_types!`.
3. Run `just write-app-server-schema` and commit the artifacts.
4. Handle the method in app-server `dispatch.rs`.
5. Add its entry to the `Methods` map in `jeeves-ts/packages/client/src/client.ts` so the typed client can call it.

## 1.3 Configuration: environment first, one overlay

`jeeves-config` has these rules:

1. **Environment variables are the source of runtime knobs.** `Settings::from_env()` reads them once at startup; `Settings` is a plain struct, with no global mutable state.
2. **Jeeves does not read `.env` files.** `.env.example` is a template to `source`.
3. **Parsing never panics and empty means unset.** `env()` trims and treats an empty string as absent. `env_bool` accepts only `1`, `true`, `yes`. Unparsable integers and floats fall back to the default; the bounds check then decides whether the final value is acceptable.
4. **Models and keys are not env knobs.** The active model, endpoint and key come from `<home>/models.json` and the credential store (`models.rs`, `vault.rs`), applied by app-server at boot. Only `JEEVES_PROVIDER` remains as the fallback vendor so a local model starts with zero config.

Field names, variables, defaults and ranges are written only in [configuration.md](../configuration.md); `scripts/check_config_docs.py` and `check_env_reads.py` fail when code and docs disagree.

### Bounds

`BOUNDS` is the one table of numeric limits. Two callers share it, so a value is judged the same way wherever it enters:

- startup, `Settings::validate`: an illegal value refuses to boot, with the variable named;
- `config/write`, `validate_patch_values`: an illegal patch is rejected wholesale.

Enumerations (`JEEVES_APPROVAL`, `JEEVES_SOUL_MODE`, `JEEVES_TOOL_POLICY`), the context-window relation (`JEEVES_MAX_TOKENS` must be below an overridden `JEEVES_CONTEXT_WINDOW`) and exec allow-rule hygiene (`MAX_EXEC_RULES`, `MAX_EXEC_RULE_CHARS`, no empty or multi-line rules) are checked in code beside the table. A knob with `patch_name: None` can only be set through the environment.

`ensure_home()` creates `traces/`, `outbox/`, `skills/` and `spill/` under `JEEVES_HOME`.

### The saved patch

The settings page needs edits that survive restart. `SettingsPatch` (a protocol type, since it crosses JSON-RPC) holds only `Option` fields and is stored as `<home>/settings.json`. It carries a small, deliberate subset: the five numeric limits, `execAllow`, `writeAllowDirs` and the three capability switches. Models and keys are excluded on purpose, so keys can never land in `settings.json`.

- `load_patch`: missing or unparsable counts as no patch, so a broken file cannot stop startup.
- `save_patch`: an empty patch deletes the file.
- `apply_patch`: only `Some` fields override.
- `SettingsPatch::merge_newer`: accumulates, with the newer patch winning per field. `is_empty` must list every field, or saving a patch that sets only one of them would delete the file.

Layering: `Settings::from_env()` → `apply_patch(saved)` → `validate()` → boot, where the models file and credentials are applied. For a patchable field the saved patch therefore beats the environment until it is cleared.

`config/write` (`apply_config_patch` in `server/settings.rs`) runs: validate → merge into the saved patch → write `settings.json` → apply to memory → re-resolve the provider → swap the settings and resolved client. Validation failure changes nothing. The file is written before memory is touched, because "changed but not remembered" misleads more than "not changed". A resolve failure is reported on stderr and leaves `turn/*` to explain the missing model.

### Invariants

- Every knob appears in `.env.example` and both configuration docs.
- Bad configuration is a startup error, never a silent default.
- A patch is validated first, persisted second, applied last.
