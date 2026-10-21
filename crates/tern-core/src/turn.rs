//! The turn state machine (`docs/03-architecture.md`, section 4.2).
//!
//! ```text
//! Created -> Preparing -> ModelStreaming -> ToolRequested -> PolicyCheck -> (ApprovalRequested)
//!         -> Executing -> RecordResult -> ... -> Verifying -> Completed
//! Terminal: Completed, CompletedWithGaps, Cancelled, Failed, Blocked, BudgetExceeded
//! ```
//!
//! Paused is not completed, and cancelled is not rolled back.

use serde::{Deserialize, Serialize};

use crate::{CoreError, CoreResult};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnState {
    Created,
    Preparing,
    ModelStreaming,
    ToolRequested,
    PolicyCheck,
    ApprovalRequested,
    Executing,
    RecordResult,
    Verifying,
    Paused,
    Completed,
    CompletedWithGaps,
    Cancelled,
    Failed,
    Blocked,
    BudgetExceeded,
}

impl TurnState {
    pub fn is_terminal(self) -> bool {
        use TurnState::*;
        matches!(
            self,
            Completed | CompletedWithGaps | Cancelled | Failed | Blocked | BudgetExceeded
        )
    }

    /// States from which the user can pause safely.
    fn pausable(self) -> bool {
        use TurnState::*;
        matches!(
            self,
            Preparing
                | ModelStreaming
                | ToolRequested
                | ApprovalRequested
                | RecordResult
                | Verifying
        )
    }

    /// Whether `self -> to` is a legal move.
    pub fn can_move_to(self, to: TurnState) -> bool {
        use TurnState::*;
        if self.is_terminal() {
            return false;
        }
        // Any live state may end in cancel, failure, or a budget stop.
        if matches!(to, Cancelled | Failed | BudgetExceeded) {
            return true;
        }
        if to == Paused {
            return self.pausable();
        }
        matches!(
            (self, to),
            (Created, Preparing)
                | (Preparing, ModelStreaming)
                | (ModelStreaming, ToolRequested)
                | (ModelStreaming, Verifying)
                | (ToolRequested, PolicyCheck)
                | (PolicyCheck, Executing)
                | (PolicyCheck, ApprovalRequested)
                | (PolicyCheck, Blocked)
                | (ApprovalRequested, Executing)
                | (ApprovalRequested, Blocked)
                | (Executing, RecordResult)
                | (RecordResult, ModelStreaming)
                | (RecordResult, ToolRequested)
                | (RecordResult, Verifying)
                | (Verifying, Completed)
                | (Verifying, CompletedWithGaps)
                | (Verifying, ModelStreaming)
                | (Paused, Preparing)
                | (Paused, ModelStreaming)
                | (Paused, ToolRequested)
                | (Paused, ApprovalRequested)
                | (Paused, RecordResult)
                | (Paused, Verifying)
        )
    }
}

/// Tracks one turn's state, refusing illegal moves.
#[derive(Clone, Debug)]
pub struct TurnStateMachine {
    state: TurnState,
    /// Where a paused turn resumes.
    resume_to: Option<TurnState>,
    history: Vec<TurnState>,
}

impl Default for TurnStateMachine {
    fn default() -> Self {
        Self {
            state: TurnState::Created,
            resume_to: None,
            history: vec![TurnState::Created],
        }
    }
}

impl TurnStateMachine {
    pub fn state(&self) -> TurnState {
        self.state
    }

    pub fn history(&self) -> &[TurnState] {
        &self.history
    }

    pub fn move_to(&mut self, to: TurnState) -> CoreResult<()> {
        if self.state.is_terminal() {
            return Err(CoreError::Terminal(self.state));
        }
        if !self.state.can_move_to(to) {
            return Err(CoreError::InvalidTransition {
                from: self.state,
                to,
            });
        }
        if self.state == TurnState::Paused
            && !matches!(
                to,
                TurnState::Cancelled | TurnState::Failed | TurnState::BudgetExceeded
            )
            && Some(to) != self.resume_to
        {
            return Err(CoreError::InvalidTransition {
                from: self.state,
                to,
            });
        }
        if to == TurnState::Paused {
            self.resume_to = Some(self.state);
        } else if self.state == TurnState::Paused {
            self.resume_to = None;
        }
        self.state = to;
        self.history.push(to);
        Ok(())
    }

    /// Resumes a paused turn where it stopped.
    pub fn resume(&mut self) -> CoreResult<()> {
        match (self.state, self.resume_to) {
            (TurnState::Paused, Some(to)) => self.move_to(to),
            (from, _) => Err(CoreError::InvalidTransition {
                from,
                to: TurnState::Preparing,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::TurnState::*;
    use super::*;

    fn run(steps: &[TurnState]) -> CoreResult<TurnStateMachine> {
        let mut m = TurnStateMachine::default();
        for s in steps {
            m.move_to(*s)?;
        }
        Ok(m)
    }

    #[test]
    fn happy_path_with_a_tool_and_an_approval() {
        let m = run(&[
            Preparing,
            ModelStreaming,
            ToolRequested,
            PolicyCheck,
            ApprovalRequested,
            Executing,
            RecordResult,
            ModelStreaming,
            Verifying,
            Completed,
        ])
        .unwrap();
        assert_eq!(m.state(), Completed);
        assert_eq!(m.history().len(), 11);
    }

    #[test]
    fn tool_cannot_skip_the_policy_gate() {
        let err = run(&[Preparing, ModelStreaming, ToolRequested, Executing]).unwrap_err();
        assert_eq!(
            err,
            CoreError::InvalidTransition {
                from: ToolRequested,
                to: Executing
            }
        );
    }

    #[test]
    fn terminal_states_are_final() {
        let mut m = run(&[Preparing, Cancelled]).unwrap();
        assert_eq!(m.move_to(Preparing), Err(CoreError::Terminal(Cancelled)));
        for t in [
            Completed,
            CompletedWithGaps,
            Cancelled,
            Failed,
            Blocked,
            BudgetExceeded,
        ] {
            assert!(t.is_terminal());
        }
    }

    #[test]
    fn pause_resumes_where_it_stopped() {
        let mut m = run(&[
            Preparing,
            ModelStreaming,
            ToolRequested,
            PolicyCheck,
            ApprovalRequested,
            Paused,
        ])
        .unwrap();
        assert_eq!(
            m.move_to(Executing).unwrap_err(),
            CoreError::InvalidTransition {
                from: Paused,
                to: Executing
            }
        );
        m.resume().unwrap();
        assert_eq!(m.state(), ApprovalRequested);
    }

    #[test]
    fn paused_is_not_completed_and_can_be_cancelled() {
        let mut m = run(&[Preparing, Paused]).unwrap();
        assert!(!m.state().is_terminal());
        m.move_to(Cancelled).unwrap();
        assert_eq!(m.state(), Cancelled);
    }

    #[test]
    fn executing_cannot_pause_mid_side_effect() {
        let err = run(&[
            Preparing,
            ModelStreaming,
            ToolRequested,
            PolicyCheck,
            Executing,
            Paused,
        ])
        .unwrap_err();
        assert_eq!(
            err,
            CoreError::InvalidTransition {
                from: Executing,
                to: Paused
            }
        );
    }

    #[test]
    fn denied_approval_blocks() {
        let m = run(&[
            Preparing,
            ModelStreaming,
            ToolRequested,
            PolicyCheck,
            ApprovalRequested,
            Blocked,
        ])
        .unwrap();
        assert_eq!(m.state(), Blocked);
    }
}
