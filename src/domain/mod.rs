pub mod account;
pub mod company;
pub mod contact;
pub mod engagement;
pub mod signal;
pub mod task;

pub use account::{
    AccountHealth, AccountStage, AccountStrategy, FLAG_CARPET_BOMB, FLAG_CONVERTED, FLAG_NEGATIVE_EVENT,
    FLAG_NEW_CONTACT_ADVANCED,
};
pub use company::{Company, HiringVelocity, normalize_domain};
pub use contact::{Contact, ContactSource, ContactStatus, Seniority};
pub use engagement::{Channel, Direction, Engagement, EngagementKind, SequenceState, StopReason};
pub use signal::{Signal, SignalKind, SignalStrength};
pub use task::{AgentTask, TaskKind, TaskStatus};
