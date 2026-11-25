#!/usr/bin/env python3
"""Synthetic stdio frames: no Runtime, network or real model."""
import json
from pathlib import Path
import subprocess
import sys
import unittest

ROOT = Path(__file__).resolve().parents[1]
CASES = json.loads((ROOT / "evals/search/public-cases.json").read_text(encoding="utf-8"))
RESPONSES = {
    "pep8-indent": ("每级用 4 个空格", "Use 4 spaces per indentation level"),
    "matthew-warning-clock": ("15:00 UTC", "1500 UTC WED OCT 05 2016"),
    "florida-groceries": ("估计上升 47.6%", "10/5/2016 | 47.6 | 14.6"),
}
CANARY = "private-test-canary"


def stream(key, mutate=lambda events: None):
    case = CASES[key]
    fact, evidence = RESPONSES[key]
    primary = (
        "Below is the body of an external web page (synthetic fixture):\n"
        f"Retrieved via Parallel MCP at synthetic-clock.\nURL: {case['source']}\n{evidence}"
    )
    events = [
        dict(type="turnStarted", userMessage=case["question"], sessionId="jeeves-main"),
        dict(type="toolStarted", toolUseId="s", tool="search_web", args=dict(query=CANARY)),
        dict(type="toolCompleted", toolUseId="s", tool="search_web", status="ok", output=CANARY),
        dict(type="toolStarted", toolUseId="f", tool="fetch_url", args=dict(url=case["source"])),
        dict(type="toolCompleted", toolUseId="f", tool="fetch_url", status="ok", output=primary),
        dict(type="turnCompleted", iterations=3, reply=f"{fact}，[来源]({case['source']})。",
             usage=dict(inputTokens=3000, outputTokens=200),
             meta=dict(interrupted=False, provider="deepseek", model="deepseek-flash",
                       retries=0, startedTasks=[])),
    ]
    mutate(events)
    frames = [dict(jsonrpc="2.0", method="turn/notification", params=dict(event, turnId="run"))
              for event in events]
    frames.append(dict(jsonrpc="2.0", id=1, result=dict(turnId="run")))
    return "\n".join(map(json.dumps, frames))


class SearchTrialTest(unittest.TestCase):
    def run_score(self, key, data):
        p = subprocess.run([sys.executable, ROOT / "scripts/score-search-trial.py", key],
                           input=data, text=True, capture_output=True, check=False)
        self.assertEqual(p.stderr, "")
        self.assertNotIn(CANARY, p.stdout)
        self.assertNotIn(CASES[key]["source"], p.stdout)
        return p.returncode, json.loads(p.stdout)

    def test_public_questions(self):
        for key in CASES:
            with self.subTest(key=key):
                code, report = self.run_score(key, stream(key))
                self.assertEqual(code, 0)
                self.assertTrue(report["pass"])
                self.assertEqual(report["search_tool_calls"], 1)
                self.assertEqual(report["fetch_tool_calls"], 1)

    def test_negated_expected_fact_is_not_correct(self):
        for key, value in [
            ("pep8-indent", "PEP 8 普通代码块不推荐每级 4 个空格"),
            ("matthew-warning-clock", "签发时间并非 15:00 UTC"),
            ("florida-groceries", "当天没有增加 47.6%"),
        ]:
            with self.subTest(key=key):
                source = CASES[key]["source"]
                edit = lambda e: e[-1].update(reply=f"{value}，[来源]({source})。")
                code, report = self.run_score(key, stream(key, edit))
                self.assertEqual(code, 1)
                self.assertFalse(report["cited_fact"])
                self.assertTrue(report["official_fetch_support"])

    def test_numeric_suffix_is_not_the_requested_fact(self):
        for key, value in [
            ("pep8-indent", "14 个空格"),
            ("pep8-indent", "4.4 个空格"),
            ("matthew-warning-clock", "11500 UTC"),
            ("matthew-warning-clock", "11:15:00 UTC"),
        ]:
            with self.subTest(key=key, value=value):
                source = CASES[key]["source"]
                edit = lambda e: e[-1].update(reply=f"{value}，[来源]({source})。")
                code, report = self.run_score(key, stream(key, edit))
                self.assertEqual(code, 1)
                self.assertFalse(report["cited_fact"])

    def test_numeric_suffix_in_fetch_does_not_verify_the_claim(self):
        key = "matthew-warning-clock"
        edit = lambda e: e[4].update(output=e[4]["output"].replace("1500 UTC", "11500 UTC"))
        code, report = self.run_score(key, stream(key, edit))
        self.assertEqual(code, 1)
        self.assertFalse(report["official_fetch_support"])

    def test_fail_closed_cases(self):
        key = "pep8-indent"
        source = CASES[key]["source"]
        edits = [
            lambda e: e[-1].update(reply=f"2 个空格，[来源]({source})"),
            lambda e: e[-1].update(reply="4 个空格，[来源](https://peps.python.org.example/pep-0008/)"),
            lambda e: e[-1].update(reply=f"4 个空格。参考：[来源]({source})"),
            lambda e: e[4].update(output=CANARY),
            lambda e: e[3]["args"].update(url=source + "?unapproved=1"),
            lambda e: e[2].update(status="error"),
            lambda e: e[-1].update(usage=None),
            lambda e: e[-1].update(usage=dict(inputTokens=18_000, outputTokens=1)),
            lambda e: e[-1].update(iterations=5),
            lambda e: e[-1]["meta"].update(startedTasks=["unexpected"]),
            lambda e: e[-1]["meta"].update(model="other-model"),
        ]
        for i, edit in enumerate(edits):
            with self.subTest(i=i):
                code, report = self.run_score(key, stream(key, edit))
                self.assertEqual(code, 1)
                self.assertFalse(report["pass"])

    def test_missing_receipt_malformed_and_oversize(self):
        key = "pep8-indent"
        for data in [CANARY, "x" * (512 * 1024 + 1), stream(key).rsplit("\n", 1)[0]]:
            code, report = self.run_score(key, data)
            self.assertEqual(code, 1)
            self.assertEqual(report["failure"], "invalid_or_incomplete_trial")


if __name__ == "__main__":
    unittest.main()
