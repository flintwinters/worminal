/* Native sealed-snapshot envelope shared by all local C/Rust ownership transactions. */
#ifndef WORMINAL_HANDOFF_FORMAT_H
#define WORMINAL_HANDOFF_FORMAT_H
#define HANDOFF_MAGIC 0x57484f31u
#define HANDOFF_MANUAL_VERSION 1u
#define HANDOFF_UPGRADE_VERSION 2u
#define HANDOFF_MAX_TABS 100
#define HANDOFF_MAX_FDS (1 + 2 * HANDOFF_MAX_TABS)
#define HANDOFF_SNAPSHOT_LIMIT (512u * 1024u * 1024u)
#define HANDOFF_TIMEOUT_SECONDS 5u
#endif
