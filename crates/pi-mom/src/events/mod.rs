use serde::{Serialize, Deserialize};
use chrono::{DateTime, Utc};

/// Event types for scheduled tasks
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ScheduledEvent {
    Immediate {
        id: String,
        description: String,
    },
    OneShot {
        id: String,
        description: String,
        trigger_at: DateTime<Utc>,
    },
    Periodic {
        id: String,
        description: String,
        cron: String,
    },
}
