//! Command palette data source: SSH servers (openWarp-only).
//!
//! In Ctrl+Shift+P the user fuzzy-matches by server name / host, and on selection → emit
//! `WorkspaceAction::OpenSshTerminal` to open a new tab and connect (via SecretInjector to
//! auto-inject the password, fully equivalent to right-click "Connect" from the SSH manager).

pub mod data_source;
pub mod search_item;

pub use data_source::SshServersDataSource;
pub use search_item::SshServerSearchItem;
