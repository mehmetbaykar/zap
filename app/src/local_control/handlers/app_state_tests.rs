use ::local_control::{ActionKind, ErrorCode};

use super::validate_staged_input_text;

#[test]
fn staged_input_rejects_line_breaks_and_control_sequences() {
    assert!(validate_staged_input_text(ActionKind::InputInsert, "safe staged text").is_ok());

    for text in ["line\nbreak", "line\rbreak", "tab\tbreak", "\u{1b}[31m"] {
        let error = validate_staged_input_text(ActionKind::InputInsert, text).err();
        assert!(error.is_some_and(|error| error.code == ErrorCode::InvalidParams));
    }
}
