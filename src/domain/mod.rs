pub mod company;
pub mod contact;
pub mod engagement;
pub mod signal;
pub mod task;

pub use company::{Company, HiringVelocity, normalize_domain};
pub use contact::{Contact, ContactSource, ContactStatus, Seniority};
pub use engagement::{Channel, Direction, Engagement, EngagementKind, SequenceState, StopReason};
pub use signal::{Signal, SignalKind, SignalStrength};
pub use task::{AgentTask, TaskKind, TaskStatus};
