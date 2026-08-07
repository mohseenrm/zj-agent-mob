//! The agent states and how each is presented.

#[derive(Copy, Clone, PartialEq, Eq)]
pub(crate) enum Status {
    Failed,
    Working,
    Waiting,
    IdleWait,
    Compact,
    Done,
    Idle,
}

impl Status {
    pub(crate) fn parse(s: &str) -> Option<Self> {
        match s {
            "working" => Some(Status::Working),
            "waiting" => Some(Status::Waiting),
            "idlewait" => Some(Status::IdleWait),
            "compact" => Some(Status::Compact),
            "failed" => Some(Status::Failed),
            "done" => Some(Status::Done),
            "idle" => Some(Status::Idle),
            _ => None,
        }
    }

    pub(crate) fn label(&self) -> &'static str {
        match self {
            Status::Working => "working",
            Status::Waiting => "waiting",
            Status::IdleWait => "idle-wait",
            Status::Compact => "compact",
            Status::Failed => "failed",
            Status::Done => "done",
            Status::Idle => "idle",
        }
    }

    /// Needs-attention first. A failure outranks a prompt: the agent is stopped,
    /// not merely blocked.
    pub(crate) fn rank(&self) -> u8 {
        match self {
            Status::Failed => 0,
            Status::Waiting => 1,
            Status::IdleWait => 2,
            Status::Done => 3,
            Status::Compact => 4,
            Status::Working => 5,
            Status::Idle => 6,
        }
    }

    /// Whether the spinner should keep ticking for this state.
    pub(crate) fn is_active(&self) -> bool {
        matches!(self, Status::Working | Status::Compact)
    }
}

impl Status {
    /// Which of Zellij's four theme colour slots this status paints with.
    ///
    /// Theme-resolved rather than a fixed 256-colour code, so the panel matches
    /// whatever palette the user runs. The mapping mirrors `ansi()`.
    pub(crate) fn color_level(&self) -> usize {
        match self {
            Status::Waiting | Status::IdleWait => 2,
            Status::Working | Status::Compact => 0,
            Status::Done => 1,
            Status::Idle | Status::Failed => 3,
        }
    }

    /// A failure is painted with the theme's error colour instead of a slot, so
    /// it cannot be confused with any working state.
    pub(crate) fn is_error(&self) -> bool {
        matches!(self, Status::Failed)
    }
}
