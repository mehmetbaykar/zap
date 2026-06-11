//! SSH password / passphrase auto-injection. Subscribes to the terminal pane's
//! PTY output broadcast and, upon matching a `password:` / `passphrase:`
//! end-of-line prompt, writes the secret + `\n` **once**.
//!
//! ## Key design tradeoffs
//!
//! - **8KB sliding window + strict end-of-line matching**: the regex
//!   `(?im)(password|passphrase)[^\n]*:\s*$` matches end-of-line only (avoiding
//!   false hits on the word "password" in motd / banner) + the sliding window
//!   guarantees a memory upper bound.
//!
//! - **15s timeout**: typical SSH public-key negotiation < 2s, password prompt
//!   < 5s. 15s is a reasonable upper bound for public-key auth failure +
//!   fallback to password. **The boundary for passwordless public-key login**
//!   (authorized_keys configured + we also stored a password): the public-key
//!   handshake succeeds → no prompt appears → the injector silently times out
//!   and exits, **without wrongly injecting into the post-login shell**.
//!
//! - **One-shot trigger**: break immediately after a match, the injector future
//!   exits → InactiveReceiver drops → subsequent PTY streams are no longer seen
//!   by this injector, **preventing a second injection**.
//!
//! - **bytes::Regex**: PTY output may contain incomplete UTF-8 bytes, so
//!   `regex::bytes` is safe.

use std::sync::Arc;
use std::time::Duration;

use async_broadcast::InactiveReceiver;
use warpui::r#async::FutureExt;
use warpui::{ViewContext, WeakViewHandle};
use zeroize::Zeroizing;

use crate::ssh_manager::password_prompt::bytes_look_like_password_prompt;
use crate::terminal::TerminalView;

/// Injection timeout upper bound.
const INJECT_TIMEOUT: Duration = Duration::from_secs(15);
/// The sliding window keeps this many of the most recent PTY output bytes for regex matching.
const SLIDING_WINDOW_BYTES: usize = 8 * 1024;
/// When the buffer exceeds this value, drain it down to the sliding-window size.
const BUFFER_HARD_LIMIT: usize = 16 * 1024;

/// Spawn a one-shot injection task in the owner=Workspace context. The task is
/// cancelled automatically when the Workspace drops; the owner doesn't need to abort.
///
/// Calling precondition: `pty_reads_rx` is obtained from
/// `terminal_view.inactive_pty_reads_rx(ctx)`, and **only starts a future when
/// it is Some**; wasm / remote sessions get None and it's a no-op.
pub fn spawn_password_injector<O>(
    pty_reads_rx: Option<InactiveReceiver<Arc<Vec<u8>>>>,
    terminal_view: WeakViewHandle<TerminalView>,
    secret: Zeroizing<String>,
    ctx: &mut ViewContext<O>,
) where
    O: warpui::View + 'static,
{
    let Some(rx) = pty_reads_rx else {
        log::debug!("ssh secret injector: no pty_reads_rx (non-local session) — skip");
        return;
    };
    if secret.is_empty() {
        log::debug!("ssh secret injector: empty secret — skip");
        return;
    }

    // Set in-flight to true on takeoff, telling the OneKey listener not to pop
    // up its menu until this injection completes. This way, whether the injector
    // finishes injecting before onekey sees the same bytes, or onekey sees them
    // first, the semantics are unified: **the injector takes priority**.
    if let Some(view) = terminal_view.upgrade(ctx) {
        view.update(ctx, |view, _| {
            view.set_ssh_secret_auto_injection_in_flight(true);
        });
    }

    let owned_secret = secret.clone();
    let future = async move {
        match watch_for_prompt(rx).with_timeout(INJECT_TIMEOUT).await {
            Ok(true) => Some(owned_secret),
            Ok(false) | Err(_) => None, // EOF or timeout → no-op
        }
    };
    ctx.spawn(future, move |_owner, secret_opt, ctx| {
        let Some(view) = terminal_view.upgrade(ctx) else {
            log::debug!("ssh secret injector: terminal view dropped before injection");
            return;
        };
        let Some(secret) = secret_opt else {
            log::debug!("ssh secret injector: no prompt seen within timeout");
            view.update(ctx, |view, _| {
                view.set_ssh_secret_auto_injection_in_flight(false);
            });
            return;
        };
        view.update(ctx, |view, ctx| {
            // Write the password + newline as bytes to the PTY, equivalent to
            // simulating keystrokes responding to the interactive prompt. At this
            // point ssh is already running (bootstrap finished long ago), so a
            // direct write_to_pty is the right answer.
            let mut bytes = secret.as_bytes().to_vec();
            bytes.push(b'\n');
            view.write_to_pty(bytes, ctx);
            view.note_ssh_secret_auto_injected(ctx);
            view.set_ssh_secret_auto_injection_in_flight(false);
        });
    });
}

/// Async loop: consume the PTY broadcast, append to the sliding window, and
/// **return true as soon as the regex matches an end-of-line prompt**; return
/// false on EOF. The timeout is wrapped by the caller via `with_timeout`.
async fn watch_for_prompt(rx: InactiveReceiver<Arc<Vec<u8>>>) -> bool {
    let mut active = rx.activate_cloned();
    let mut buf: Vec<u8> = Vec::with_capacity(SLIDING_WINDOW_BYTES);
    while let Ok(chunk) = active.recv().await {
        buf.extend_from_slice(&chunk);
        if buf.len() > BUFFER_HARD_LIMIT {
            let drop_n = buf.len() - SLIDING_WINDOW_BYTES;
            buf.drain(..drop_n);
        }
        if bytes_look_like_password_prompt(&buf) {
            return true;
        }
    }
    false
}
