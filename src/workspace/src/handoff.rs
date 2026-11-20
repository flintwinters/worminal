//! Encode and decode messages for transferring PTYs between daemons.

// Sealed-snapshot envelope shared by every local ownership transaction, including installed peers.
pub const HANDOFF_MAGIC: u32 = 0x5748_4f31;
pub const HANDOFF_MANUAL_VERSION: u32 = 1;
pub const HANDOFF_UPGRADE_VERSION: u32 = 2;
/// Oldest upgrade snapshot a receiver still reads. The donor is the already-running (older) binary,
/// so a receiver must read every layout any release has ever sent; raise this never, only extend
/// `HANDOFF_UPGRADE_VERSION` additively and keep a reader for each earlier layout.
pub const HANDOFF_UPGRADE_MIN_VERSION: u32 = 2;
pub const HANDOFF_MAX_TABS: u32 = 100;
pub const HANDOFF_MAX_FDS: u32 = 1 + 2 * HANDOFF_MAX_TABS;
pub const HANDOFF_SNAPSHOT_LIMIT: u32 = 512 * 1024 * 1024;
pub const HANDOFF_TIMEOUT_SECONDS: u32 = 5;

#[cfg(test)]
mod tests {
    use super::*;

    /// Shipped donors send these exact values; changing them strands every running owner that has
    /// them, because only a restart (never an update) can replace a donor.
    #[test]
    fn shipped_snapshot_identity_is_frozen() {
        assert_eq!(HANDOFF_MAGIC, 0x5748_4f31);
        assert_eq!(HANDOFF_MANUAL_VERSION, 1);
        assert_eq!(HANDOFF_UPGRADE_MIN_VERSION, 2);
        assert!((HANDOFF_UPGRADE_MIN_VERSION..=HANDOFF_UPGRADE_VERSION).contains(&2));
    }
}
