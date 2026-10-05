//! Synchronous Insta360 Wave USB controls and a separate cloud binding-key lookup.

/// Read-only Insta360 binding-key retrieval; USB controls never call this module.
pub mod wave_binding_key;
/// Wave device discovery and synchronous USB command exchange.
pub mod wave_client;
/// Packed command requests and replies for Wi-Fi, meetings, settings, and accounts.
pub mod wave_command_protocol;
/// Recovered HID, MAVLink, PC-Link, and packed device formats.
pub mod wave_protocol;
