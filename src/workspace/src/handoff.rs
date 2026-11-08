//! Encode and decode messages for transferring PTYs between daemons.

// Sealed-snapshot envelope shared by every local ownership transaction, including installed peers.
pub const HANDOFF_MAGIC: u32 = 0x5748_4f31;
pub const HANDOFF_MANUAL_VERSION: u32 = 1;
pub const HANDOFF_UPGRADE_VERSION: u32 = 2;
pub const HANDOFF_MAX_TABS: u32 = 100;
pub const HANDOFF_MAX_FDS: u32 = 1 + 2 * HANDOFF_MAX_TABS;
pub const HANDOFF_SNAPSHOT_LIMIT: u32 = 512 * 1024 * 1024;
pub const HANDOFF_TIMEOUT_SECONDS: u32 = 5;
