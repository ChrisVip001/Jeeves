#!/usr/bin/env python3
"""Score a single private Jeeves app-server stream without echoing its contents."""
import argparse
import json
from pathlib import Path
import re
import sys

CASES = Path(__file__).resolve().parents[1] / "evals/search/public-cases.json"
MAX_LINE = 512 * 1024
MAX_BYTES = 8 * 1024 * 1024
MAX_TOKENS = 18_000
TOOLS = ["search_web", "fetch_url"]


def score(case, stream):
    starts, ends, tools = {}, {}, []
    turn = done = None
    bytes_read = retries = 0
    receipt = False
    while raw := stream.readline(MAX_LINE + 1):
        bytes_read += len(raw)
        if len(raw) > MAX_LINE or bytes_read > MAX_BYTES or receipt:
            raise ValueError
        f = json.loads(raw)
        if f["jsonrpc"] != "2.0" or "error" in f:
            raise ValueError
        if f.get("method") != "turn/notification":
            if not done or f["id"] != 1 or f["result"]["turnId"] != turn:
                raise ValueError
            receipt = True
            continue
        p = f["params"]
        kind = p["type"]
        if kind == "turnStarted":
            if (turn is not None or not isinstance(p["turnId"], str)
                or p["sessionId"] != "jeeves-main" or p["userMessage"] != case["question"]):
                raise ValueError
            turn = p["turnId"]
        if kind in {"toolStarted", "toolCompleted", "turnCompleted", "retry"} and (
            turn is None or p.get("turnId") != turn or done is not None
        ):
            raise ValueError
        if kind == "toolStarted":
            use = p["toolUseId"]
            if use in starts:
                raise ValueError
            starts[use] = p
            tools.append(p["tool"])
        elif kind == "toolCompleted":
            use = p["toolUseId"]
            if use not in starts or use in ends or starts[use]["tool"] != p["tool"]:
                raise ValueError
            ends[use] = p
        elif kind == "turnCompleted":
            done = p
        elif kind == "retry":
            retries += 1
        elif kind in {"approvalRequested", "taskUpdated", "error", "turnResumed", "turnSuspended"}:
            raise ValueError
    if not receipt or starts.keys() != ends.keys():
        raise ValueError

    source = case["source"]
    links = [f"]({source})"]
    if source == "https://peps.python.org/pep-0008/":
        links.append(f"]({source}#indentation)")
    reply, meta, iterations = done["reply"], done["meta"], done["iterations"]
    if type(iterations) is not int or iterations < 0 or not isinstance(meta["startedTasks"], list):
        raise ValueError
    matches_answer = any(
        any(link in clause for link in links) and re.search(case["answer"], clause, re.I)
        for clause in re.split(r"[。！？\n]", reply)
    ) and not re.search(case["reject"], reply, re.I)
    def matches_source_output(text):
        intro, _, source_text = text.partition("\n")
        return (
            intro.startswith("Below is the body of an external web page (")
            and source_text.startswith("Retrieved via Parallel MCP at ")
            and f"URL: {source}" in source_text.splitlines()
            and re.search(case["evidence"], source_text, re.I) is not None
        )

    matches_fetch = any(
        start["tool"] == "fetch_url" and start["args"]["url"] == source
        and ends[use]["status"] == "ok" and matches_source_output(ends[use]["output"])
        for use, start in starts.items()
    )
    usage = done.get("usage")
    tokens = None
    if usage is not None:
        values = [usage[key] for key in ("inputTokens", "outputTokens")]
        if any(type(n) is not int or n < 0 for n in values):
            raise ValueError
        if sum(values):
            tokens = dict(zip(("input_tokens", "output_tokens"), values))
    expected_model = meta["provider"] == "deepseek" and meta["model"] == "deepseek-flash"
    clean = meta["interrupted"] is False and not meta["startedTasks"] and retries == meta["retries"] == 0
    ordered = tools == TOOLS and all(end["status"] == "ok" for end in ends.values())
    budget = tokens is not None and sum(tokens.values()) <= MAX_TOKENS and 1 <= iterations <= 4
    return {
        "pass": bool(matches_answer and matches_fetch and expected_model and clean and ordered and budget),
        "cited_fact": bool(matches_answer),
        "official_fetch_support": matches_fetch,
        "expected_model": expected_model,
        "clean_finish": clean,
        "ordered_search_then_fetch": ordered,
        "within_trial_budget": budget,
        "search_tool_calls": tools.count("search_web"),
        "fetch_tool_calls": tools.count("fetch_url"),
        "other_tool_calls": len(tools) - sum(tools.count(tool) for tool in TOOLS),
        "model_loop_iterations": iterations,
        "retry_notifications": retries,
        "turn_usage": tokens,
    }


def main():
    cases = json.loads(CASES.read_text(encoding="utf-8"))
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("case", choices=cases)
    args = parser.parse_args()
    try:
        report = score(cases[args.case], sys.stdin.buffer)
    except (AttributeError, IndexError, KeyError, RecursionError, TypeError, ValueError):
        report = {"pass": False, "failure": "invalid_or_incomplete_trial"}
    print(json.dumps(report, sort_keys=True))
    return 0 if report["pass"] else 1


if __name__ == "__main__":
    sys.exit(main())
