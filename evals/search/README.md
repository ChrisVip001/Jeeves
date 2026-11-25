# First-answer search regression

Run `cargo test --locked -p jeeves-app-server --lib interaction::tests::web_lookup`.

Three fictional Chinese lookups cover stale opening hours, similarly named transit lines, and a date the publisher has not announced. They drive the main-chat interaction and real web tool wrappers with scripted adapters. The checks require one reply, the expected fact with an attached source link, literal support in the fetch output, one search and one fetch at the fixture seam, and zero background tasks. Negative controls cover stale facts, wrong or detached links, missing evidence, extra calls, and task creation.

Three additional fictional flight fixtures replay a calendar minimum next to a wrong-day offer,
a matching outbound leg priced as part of a round trip, and a self-contained one-way quote next
to a cheaper reverse route. They check the fetched evidence and which offer-scope guidance
reaches the model. Scripted replies exercise partial results, indicative prices, and unknown
booking terms separately. All prices and flight identifiers are test data.

No network, key, or real model is needed. This checks wiring and literal assertions, not whether
a live model follows the guidance, how production search ranks sites, or whether a flight is
bookable.

## Optional live pilot, not run by this repository check

The public-cases manifest freezes three historical/reference questions, exact official pages, evidence patterns, and known wrong-answer patterns. It is independent of the fictional wiring fixtures. Do not put the answers, expected links, or grading rubric into the model session. These cases are a small smoke pilot, not a representative quality claim.

1. Only after the operator permits live model usage, build the pinned revision and use one disposable Runtime home and fresh process per case, outside the repository. Start with **one** case and no automatic repeat. Use a restricted process environment: no copied user content, profiles, plugins, notifications, or connectors. Disable writes, command execution, and delegation. The only credential is a DeepSeek trial key supplied as the existing environment-only profile `/model add ds deepseek deepseek-flash --env DEEPSEEK_API_KEY`; never place it in shell arguments, request JSON, files, or logs. Select the profile before the trial. Do not use an existing personal Runtime home.
2. For a one-case pilot set `JEEVES_MODEL_ROUTE=fixed`, `JEEVES_MAX_TOKENS=320`, `JEEVES_TURN_TOKEN_BUDGET=18000`, `JEEVES_LLM_RETRIES=0`, `JEEVES_LLM_TIMEOUT=25` and `JEEVES_SEARCH_MAX_PER_TURN=1`. Use only the approved provider/search endpoints and set an external process deadline (for example 90 seconds). If a hard spending cap is required, use a verified provider-side limit or do not run. The output limit is per request; the turn setting is an estimated wrap-up threshold, and the deadline cannot undo usage already incurred.
3. With the established profile start `jeeves app-server` in the disposable environment. Send just one newline-delimited JSON-RPC `interaction/start` request with `id: 1`, `sessionId: jeeves-main`, a fresh `messageId`, and the selected verbatim question. Keep stdin open until the matching response, then close it. The host should stop the trial at an unexpected tool, task, approval or deadline; no authorization to execute actions follows from a benchmark prompt. Score that one request's stdout through `python3 scripts/score-search-trial.py <case-id>`, feeding it as a private stream rather than a stored/public transcript. Do not mix setup RPC frames, other requests, stderr, or shell xtrace into it.
4. Share only the aggregate scorer output with the revision and trial policy. The scorer emits counts and booleans, not the reply, argument strings, tool output, or error details. Runtime can still write internal logs, the chat database and traces (including user content), so keep the disposable home private and do not attach or commit it. Treat outputs with missing/zero usage as unavailable. A reported nonzero total is the Runtime's turn total, not a provider billing receipt. Tool starts count user-facing tool calls: internal backend fallbacks and provider HTTP attempts are not visible here.

The checker can be exercised entirely offline with `python3 scripts/test-score-search-trial.py`. It accepts only focused search-then-fetch cases with a matching structured Parallel source URL and predefined evidence (the built-in extractor does not expose its final redirect URL to this stream), not all semantically correct phrasings. It does not itself run or spend anything; an isolated host/client must be arranged separately before live use.

Interpret the component fields separately from the strict fast-path flag. For example, the official
Fed article can lead to the required accessible table via its Figure 5 link. A second fetch of that
table can establish the expected fact and evidence while failing the one-fetch fast-path criterion.
For the NOAA question, the official Public Advisory and Forecast/Advisory are different documents
even when they report the same time. A matching number and unverified citation are not proof that
the requested product was read. Treat blocked source access as an evidence issue, distinct from an
incorrect answer; use human review for source paths and phrasings outside the fixed checker.
