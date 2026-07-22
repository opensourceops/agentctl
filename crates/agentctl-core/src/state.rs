use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    Running,
    Paused,
    Succeeded,
    Failed,
    Cancelled,
}

impl RunState {
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }

    pub fn transition(self, next: Self) -> Result<Self, InvalidRunTransition> {
        let valid = matches!(
            (self, next),
            (
                Self::Running,
                Self::Paused | Self::Succeeded | Self::Failed | Self::Cancelled
            ) | (Self::Paused, Self::Running | Self::Failed | Self::Cancelled)
        );
        if valid {
            Ok(next)
        } else {
            Err(InvalidRunTransition {
                from: self,
                to: next,
            })
        }
    }
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
#[error("invalid run transition from {from:?} to {to:?}")]
pub struct InvalidRunTransition {
    pub from: RunState,
    pub to: RunState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Pending,
    Ready,
    Running,
    WaitingForApproval,
    WaitingForEffect,
    RetryScheduled,
    Succeeded,
    Failed,
    Skipped,
    Cancelled,
}

impl TaskState {
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::Skipped | Self::Cancelled
        )
    }

    pub fn transition(self, next: Self) -> Result<Self, InvalidTransition> {
        let valid = matches!(
            (self, next),
            (Self::Pending, Self::Ready | Self::Skipped | Self::Cancelled)
                | (Self::Ready, Self::Running | Self::Cancelled)
                | (
                    Self::Running,
                    Self::WaitingForApproval
                        | Self::WaitingForEffect
                        | Self::Succeeded
                        | Self::Failed
                        | Self::RetryScheduled
                        | Self::Cancelled
                )
                | (
                    Self::WaitingForApproval,
                    Self::Running | Self::Failed | Self::Cancelled
                )
                | (
                    Self::WaitingForEffect,
                    Self::Running | Self::Failed | Self::Cancelled
                )
                | (Self::RetryScheduled, Self::Ready | Self::Cancelled)
        );
        if valid {
            Ok(next)
        } else {
            Err(InvalidTransition {
                from: self,
                to: next,
            })
        }
    }
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
#[error("invalid task transition from {from:?} to {to:?}")]
pub struct InvalidTransition {
    pub from: TaskState,
    pub to: TaskState,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_terminal_state_rejects_every_transition() {
        let states = [
            TaskState::Pending,
            TaskState::Ready,
            TaskState::Running,
            TaskState::WaitingForApproval,
            TaskState::WaitingForEffect,
            TaskState::RetryScheduled,
            TaskState::Succeeded,
            TaskState::Failed,
            TaskState::Skipped,
            TaskState::Cancelled,
        ];
        for terminal in [
            TaskState::Succeeded,
            TaskState::Failed,
            TaskState::Skipped,
            TaskState::Cancelled,
        ] {
            for candidate in states {
                assert!(terminal.transition(candidate).is_err());
            }
        }
    }

    #[test]
    fn happy_path_is_explicit() {
        let state = TaskState::Pending
            .transition(TaskState::Ready)
            .and_then(|state| state.transition(TaskState::Running))
            .and_then(|state| state.transition(TaskState::WaitingForEffect))
            .and_then(|state| state.transition(TaskState::Running))
            .and_then(|state| state.transition(TaskState::Succeeded))
            .expect("valid path");
        assert_eq!(state, TaskState::Succeeded);
    }

    #[test]
    fn run_transitions_are_explicit_and_terminal_runs_are_immutable() {
        assert_eq!(
            RunState::Running.transition(RunState::Paused),
            Ok(RunState::Paused)
        );
        assert_eq!(
            RunState::Paused.transition(RunState::Running),
            Ok(RunState::Running)
        );
        for terminal in [RunState::Succeeded, RunState::Failed, RunState::Cancelled] {
            assert!(terminal.is_terminal());
            for candidate in [
                RunState::Running,
                RunState::Paused,
                RunState::Succeeded,
                RunState::Failed,
                RunState::Cancelled,
            ] {
                assert!(terminal.transition(candidate).is_err());
            }
        }
    }
}
