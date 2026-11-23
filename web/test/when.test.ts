import assert from "node:assert/strict";
import { test } from "node:test";
import { readableMinute } from "../src/when.ts";

const now = new Date(2026, 10, 24, 10, 30);

test("near minutes read as today, tomorrow, yesterday or a weekday", () => {
  assert.equal(readableMinute("2026-11-24 15:00", now), "今天 15:00");
  assert.equal(readableMinute("2026-11-25 09:00", now), "明天 09:00");
  assert.equal(readableMinute("2026-11-23 08:05", now), "昨天 08:05");
  assert.equal(readableMinute("2026-11-27 08:00", now), "周五 08:00");
  assert.equal(readableMinute("2026-11-24T07:00", now), "今天 07:00");
});

test("far minutes keep a date, with the year only when it differs", () => {
  assert.equal(readableMinute("2026-12-01 08:00", now), "12-01 08:00");
  assert.equal(readableMinute("2027-01-05 08:00", now), "2027-01-05 08:00");
  assert.equal(readableMinute("2026-11-20 08:00", now), "11-20 08:00");
});

test("anything that is not a minute passes through unchanged", () => {
  for (const raw of ["", "0 8 * * 1-5", "2026-11-24", "this cron will not fire within a year"]) {
    assert.equal(readableMinute(raw, now), raw);
  }
});
