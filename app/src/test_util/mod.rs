pub mod ai_agent_tasks;
pub mod blockgrid;
pub mod settings;
pub mod terminal;
mod virtual_fs;

pub use blockgrid::mock_blockgrid;
pub use terminal::add_window_with_terminal;
pub use virtual_fs::{Stub, VirtualFS};

macro_rules! assert_eventually {
    // 60 ticks x 5ms = a 300ms budget. The old default of 20 (100ms) was too
    // tight for the Windows CI runner, whose suite takes ~735s against Linux's
    // ~120s: `submit_cli_agent_rich_input_opencode_defers_enter_and_close` waits
    // on a real delayed timer and failed all three retries there while passing on
    // macOS and Linux. Raising the cap costs nothing when the assertion holds --
    // the loop breaks on the first successful poll -- and only lengthens how long
    // a genuinely failing assertion takes to report.
    ($cond:expr_2021, $($arg:tt)+) => {
        $crate::test_util::assert_eventually!(60 => $cond, $($arg)+);
    };
    // Run the condition up to ticks times, yielding to the executor in between.  If it does
    // not become true, this panics with the provided format string + args.
    ($ticks:literal => $cond:expr_2021, $($arg:tt)+) => {{
        let mut pass = false;
        for _ in 0..$ticks {
            if $cond {
                pass = true;
                break;
            }
            warpui::r#async::Timer::after(std::time::Duration::from_millis(5)).await;
        }
        if !pass {
            panic!("{}", format_args!($($arg)+));
        }
    }};
}
pub(crate) use assert_eventually;
