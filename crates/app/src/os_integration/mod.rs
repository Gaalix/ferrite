//! OS-level media controls (Linux MPRIS2 / Windows SMTC) and display-sleep inhibition.

pub mod media_controls;
pub mod sleep_inhibitor;

pub use media_controls::{OsMediaCommand, OsMediaController, OsMediaMetadata, OsPlaybackStatus};
pub use sleep_inhibitor::{should_inhibit_display_sleep, SleepInhibitor};
