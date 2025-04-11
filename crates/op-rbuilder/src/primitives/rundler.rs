use std::fmt::Display;
use rundler_builder::{BuilderEvent, BuilderEventKind};
use rundler_pool::PoolEvent;
use rundler_utils::emit::WithEntryPoint;

#[derive(Clone, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum Event {
    PoolEvent(PoolEvent),
    BuilderEvent(BuilderEvent),
}

impl From<PoolEvent> for Event {
    fn from(event: PoolEvent) -> Self {
        Self::PoolEvent(event)
    }
}

impl From<BuilderEvent> for Event {
    fn from(event: BuilderEvent) -> Self {
        Self::BuilderEvent(event)
    }
}

impl Display for Event {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Event::PoolEvent(event) => event.fmt(f),
            Event::BuilderEvent(event) => event.fmt(f),
        }
    }
}



/// This function is taker from rundler-cli crate `bin/rundler/src/cli/builder.rs`
pub fn is_nonspammy_event(event: &WithEntryPoint<BuilderEvent>) -> bool {
    if let BuilderEventKind::FormedBundle {
        tx_details,
        fee_increase_count,
        ..
    } = &event.event.kind
    {
        if tx_details.is_none() && *fee_increase_count == 0 {
            return false;
        }
    }
    true
}