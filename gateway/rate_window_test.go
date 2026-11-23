package main

import (
	"testing"
	"time"
)

func TestRetryAfterIsTheTimeLeftInTheRateWindow(t *testing.T) {
	base := time.Unix(1_800_000_000-1_800_000_000%60, 0)
	for offset, want := range map[int]int{0: 60, 1: 59, 45: 15, 59: 1} {
		if got := untilNextWindow(base.Add(time.Duration(offset) * time.Second)); got != want {
			t.Errorf("%ds into the window: got %d, want %d", offset, got, want)
		}
	}
}
