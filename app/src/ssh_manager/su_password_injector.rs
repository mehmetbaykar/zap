//! su password confirmation prompt. Continuously listens to PTY output, and when
//! it detects a password prompt appearing after the user enters a switch-to-root
//! command like `su root` / `su - root`, pops up a confirmation menu and injects
//! the root password after the user confirms.
//!
//! Only injects for the root target; switching to other users like `su lg` does
//! not trigger it. It first waits for the shell prompt to appear (indicating SSH
//! login has completed) before starting detection, to avoid conflicting with the
//! login password. It uses `spawn_stream_local` + `stream!` for continuous
//! listening, triggering on every `su root`.

use std::sync::Arc;
use std::time::Duration;

use async_broadcast::InactiveReceiver;
use async_stream::stream;
use lazy_static::lazy_static;
use regex::bytes::Regex;
use warpui::r#async::FutureExt;
use warpui::{ViewContext, WeakViewHandle};
use zeroize::Zeroizing;

use crate::ssh_manager::shell_prompt::bytes_look_like_shell_prompt;
use crate::terminal::TerminalView;

const SLIDING_WINDOW_BYTES: usize = 8 * 1024;
const BUFFER_HARD_LIMIT: usize = 16 * 1024;
/// Maximum duration for phase 1 to wait for the shell prompt. On timeout, give
/// up the entire stream (and reset in_flight in `on_done`).
const SHELL_READY_TIMEOUT: Duration = Duration::from_secs(30);

lazy_static! {
    /// Password prompt regex — strictly matches two categories:
    /// 1. `password` / `passphrase` / `密码` at end-of-line with a halfwidth
    ///    colon `:` or fullwidth colon `：`
    /// 2. Galaxy Kylin V10's colon-less `输入密码`
    ///
    /// The old implementation made the colon optional, so any end-of-line
    /// containing "password" (e.g. `Your password has expired`) would false-positive.
    static ref PASSWORD_PROMPT_REGEX: Regex = Regex::new(
        r"(?im)(?:(?:password|passphrase|密码)[^\n]*(?::|：)\s*$|输入密码\s*$)"
    )
    .expect("su password prompt regex must compile");

    /// su command regex — matches su commands targeting root (at end-of-line):
    /// `su` / `su -` / `su -l` / `su --login` / `su root` / `su - root` /
    /// `su -l root` / `su --login root`. Does not match switch-to-other-user
    /// forms like `su lg` / `su - lg`; `sudo su` still matches the trailing `su`
    /// thanks to the `\bsu` word boundary.
    static ref SU_ROOT_CMD_REGEX: Regex =
        Regex::new(r"(?m)\bsu(?:\s+(?:-l?|--login|-))*(?:\s+root)?\s*$")
            .expect("su root cmd regex must compile");
}

/// Spawn the su password continuous-listening stream in the owner context.
pub fn spawn_su_password_injector<O>(
    pty_reads_rx: Option<InactiveReceiver<Arc<Vec<u8>>>>,
    terminal_view: WeakViewHandle<TerminalView>,
    root_password: Zeroizing<String>,
    ctx: &mut ViewContext<O>,
) where
    O: warpui::View + 'static,
{
    let Some(rx) = pty_reads_rx else {
        log::debug!("ssh su password injector: no pty_reads_rx — skip");
        return;
    };
    if root_password.is_empty() {
        log::debug!("ssh su password injector: empty root password — skip");
        return;
    }

    // Set the in-flight flag to prevent the OneKey credential picker from popping up while waiting for the shell prompt.
    if let Some(view) = terminal_view.upgrade(ctx) {
        view.update(ctx, |view, _| {
            view.set_ssh_secret_auto_injection_in_flight(true);
        });
    }

    let prompt_stream = stream! {
        let mut active = rx.activate_cloned();
        let mut buf: Vec<u8> = Vec::with_capacity(SLIDING_WINDOW_BYTES);

        // Phase 1: wait for the shell prompt (SHELL_READY_TIMEOUT timeout), indicating login is complete
        loop {
            match active.recv().with_timeout(SHELL_READY_TIMEOUT).await {
                Ok(Ok(chunk)) => {
                    buf.extend_from_slice(&chunk);
                    if buf.len() > BUFFER_HARD_LIMIT {
                        let drop_n = buf.len() - SLIDING_WINDOW_BYTES;
                        buf.drain(..drop_n);
                    }
                    if bytes_look_like_shell_prompt(&buf) {
                        break;
                    }
                }
                _ => return,
            }
        }

        // Phase 2: continuously detect su root + password prompt, keep listening after each yield
        buf.clear();
        while let Ok(chunk) = active.recv().await {
            buf.extend_from_slice(&chunk);
            if buf.len() > BUFFER_HARD_LIMIT {
                let drop_n = buf.len() - SLIDING_WINDOW_BYTES;
                buf.drain(..drop_n);
            }
            if PASSWORD_PROMPT_REGEX.is_match(&buf) && is_su_to_root(&buf) {
                buf.clear();
                yield ();
            }
        }
    };

    // on_done must reset in_flight: if phase 1 (waiting for the shell prompt)
    // times out / hits EOF and `return`s out of the stream directly, on_item has
    // not yet run, and if we don't reset it in on_done, OneKey would be blocked
    // permanently in that terminal.
    let terminal_view_done = terminal_view.clone();
    let _ = ctx.spawn_stream_local(
        prompt_stream,
        move |_owner, (), ctx| {
            let Some(view) = terminal_view.upgrade(ctx) else {
                return;
            };
            view.update(ctx, |view, ctx| {
                view.su_root_password = Some(root_password.clone());
                view.show_su_root_confirm_menu(ctx);
                view.set_ssh_secret_auto_injection_in_flight(false);
            });
        },
        move |_owner, ctx| {
            if let Some(view) = terminal_view_done.upgrade(ctx) {
                view.update(ctx, |view, _| {
                    view.set_ssh_secret_auto_injection_in_flight(false);
                });
            }
        },
    );
}

/// Check whether the buffer contains a su command targeting root.
fn is_su_to_root(buf: &[u8]) -> bool {
    SU_ROOT_CMD_REGEX.is_match(buf)
}

#[cfg(test)]
#[path = "su_password_injector_tests.rs"]
mod tests;
