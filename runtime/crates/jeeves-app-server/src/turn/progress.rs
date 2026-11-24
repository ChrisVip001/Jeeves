//! Stops a task that keeps correcting without getting anywhere, instead of spending every
//! remaining round on the same failure.

/// Consecutive stalled correction rounds after which the task goes back to the person.
const MAX_STALLS: u32 = 2;

/// Tracks the feedback each round of one run ended with.
#[derive(Default)]
pub(super) struct Progress {
    last: Option<String>,
    stalls: u32,
}

impl Progress {
    /// Records the feedback a finished round ended with and whether it ran any tool successfully.
    /// The first round only sets the baseline. A later round stalls when it ends with the same
    /// feedback as the round before it or did no work; any other round resets the count.
    /// Returns true once `MAX_STALLS` stalled rounds come in a row.
    pub(super) fn stalled(&mut self, feedback: &str, worked: bool) -> bool {
        let feedback = normalise(feedback);
        if let Some(last) = &self.last {
            if *last == feedback || !worked {
                self.stalls += 1;
            } else {
                self.stalls = 0;
            }
        }
        self.last = Some(feedback);
        self.stalls >= MAX_STALLS
    }
}

fn normalise(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::Progress;

    #[test]
    fn the_same_failure_twice_after_the_first_round_is_a_stall() {
        let mut progress = Progress::default();
        assert!(!progress.stalled("notes.md lacks Agenda", true));
        assert!(!progress.stalled("notes.md  lacks agenda", true));
        assert!(progress.stalled("notes.md lacks Agenda", true));
    }

    #[test]
    fn a_round_that_did_nothing_stalls_even_with_new_feedback() {
        let mut progress = Progress::default();
        assert!(!progress.stalled("first", false));
        assert!(!progress.stalled("second", false));
        assert!(progress.stalled("third", false));
    }

    #[test]
    fn a_round_with_new_feedback_and_work_resets_the_count() {
        let mut progress = Progress::default();
        assert!(!progress.stalled("a", true));
        assert!(!progress.stalled("a", true));
        assert!(!progress.stalled("b", true));
        assert!(!progress.stalled("b", true));
        assert!(progress.stalled("b", true));
    }
}
