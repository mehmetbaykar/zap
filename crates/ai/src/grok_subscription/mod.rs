//! Provider-direct xAI / Grok authentication.
//!
//! This module intentionally contains only the OAuth protocol flow. Token
//! persistence and request routing must remain local and must never package
//! credentials for a remote proxy API.

pub mod oauth;
