use super::slash_command_is_submitted_as_prompt;
use crate::search::slash_command_menu::static_commands::commands;

/// The centralized classifier must mark only the prompt-submitting commands (/compact, /plan,
/// /orchestrate) as "submitted as a prompt". Every other slash command emits an immediate action
/// and must be treated as "run now" by the prompt-queue gate and the shared-session viewer path.
#[test]
fn slash_command_is_submitted_as_prompt_only_for_prompt_commands() {
    // Prompt-submitting commands reiterate their text into the conversation.
    assert!(slash_command_is_submitted_as_prompt(&commands::COMPACT));
    assert!(slash_command_is_submitted_as_prompt(&commands::PLAN));
    assert!(slash_command_is_submitted_as_prompt(&commands::ORCHESTRATE));

    // Action-emitting commands run immediately and are never queued / forwarded as prompts.
    assert!(!slash_command_is_submitted_as_prompt(&commands::FORK));
    assert!(!slash_command_is_submitted_as_prompt(
        &commands::FORK_AND_COMPACT
    ));
    assert!(!slash_command_is_submitted_as_prompt(&commands::FORK_FROM));
    assert!(!slash_command_is_submitted_as_prompt(
        &commands::COMPACT_AND
    ));
    assert!(!slash_command_is_submitted_as_prompt(&commands::MODEL));
    assert!(!slash_command_is_submitted_as_prompt(&commands::REWIND));
    assert!(!slash_command_is_submitted_as_prompt(
        &commands::CONVERSATIONS
    ));
    assert!(!slash_command_is_submitted_as_prompt(&commands::QUEUE));
}

#[cfg(all(feature = "local_fs", windows))]
mod windows {
    use std::sync::Arc;

    use super::super::*;
    use crate::terminal::model::session::command_executor::testing::TestCommandExecutor;
    use crate::terminal::model::session::SessionInfo;
    use crate::terminal::shell::ShellType;
    use crate::terminal::ShellLaunchData;

    fn wsl_session() -> Session {
        Session::new(
            SessionInfo::new_for_test().with_shell_type(ShellType::Bash),
            Arc::new(TestCommandExecutor::default()),
        )
        .with_shell_launch_data(ShellLaunchData::WSL {
            distro: "Ubuntu".to_owned(),
        })
    }

    #[test]
    fn open_file_command_converts_wsl_paths_to_host_paths() {
        let session = wsl_session();
        let cases = [
            (
                "/home/ubuntu",
                "subdir/test.txt",
                r"\\WSL$\Ubuntu\home\ubuntu\subdir\test.txt",
                None,
            ),
            (
                "/home/ubuntu/project",
                "../test.txt",
                r"\\WSL$\Ubuntu\home\ubuntu\test.txt",
                None,
            ),
            (
                "/home/ubuntu",
                "subdir/file\\ name.txt",
                r"\\WSL$\Ubuntu\home\ubuntu\subdir\file name.txt",
                None,
            ),
            (
                "/home/ubuntu",
                "subdir/test.txt:4:2",
                r"\\WSL$\Ubuntu\home\ubuntu\subdir\test.txt",
                Some(LineAndColumnArg {
                    line_num: 4,
                    column_num: Some(2),
                }),
            ),
        ];

        for (current_dir, raw_arg, expected_path, expected_line_col) in cases {
            let (path, line_col) = open_file_command_path(&session, current_dir, raw_arg);

            assert_eq!(path, PathBuf::from(expected_path));
            assert_eq!(line_col, expected_line_col);
        }
    }
}
