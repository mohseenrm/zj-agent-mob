//! Agent status: the four states an agent can be in, and how each is presented.

use crate::style::{BLUE, GREEN, GREY, RED};

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

    /// Sort order: things needing attention first.
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
    pub(crate) fn ansi(&self) -> &'static str {
        match self {
            Status::Waiting => RED,
            Status::Working => BLUE,
            Status::Done => GREEN,
            Status::Idle => GREY,
        }
    }
}
