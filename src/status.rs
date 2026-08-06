//! The four agent states and how each is presented.

#[derive(Copy, Clone, PartialEq, Eq)]
pub(crate) enum Status {
    Working,
    Waiting,
    Done,
    Idle,
}

impl Status {
    pub(crate) fn parse(s: &str) -> Option<Self> {
        match s {
            "working" => Some(Status::Working),
            "waiting" => Some(Status::Waiting),
            "done" => Some(Status::Done),
            "idle" => Some(Status::Idle),
            _ => None,
        }
    }

    pub(crate) fn label(&self) -> &'static str {
        match self {
            Status::Working => "working",
            Status::Waiting => "waiting",
            Status::Done => "done",
            Status::Idle => "idle",
        }
    }

    /// Needs-attention first.
    pub(crate) fn rank(&self) -> u8 {
        match self {
            Status::Waiting => 0,
            Status::Done => 1,
            Status::Working => 2,
            Status::Idle => 3,
        }
    }
}

impl Status {
    /// Which of Zellij's four theme colour slots this status paints with.
    ///
    /// Theme-resolved rather than a fixed 256-colour code, so the panel matches
    /// whatever palette the user runs. The mapping mirrors `ansi()`.
    pub(crate) fn color_level(&self) -> usize {
        match self {
            Status::Waiting => 2,
            Status::Working => 0,
            Status::Done => 1,
            Status::Idle => 3,
        }
    }

}
