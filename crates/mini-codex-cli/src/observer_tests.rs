use super::*;

#[test]
fn colors_only_the_terminal_tag() {
    assert_eq!(
        styled_tag("thinking>", TagColor::Magenta, true),
        "\u{1b}[35mthinking>\u{1b}[0m"
    );
    assert_eq!(
        styled_tag("tool[ok]>", TagColor::Green, true),
        "\u{1b}[32mtool[ok]>\u{1b}[0m"
    );
}

#[test]
fn leaves_tags_plain_when_color_is_disabled() {
    assert_eq!(
        styled_tag("tool[error]>", TagColor::Red, false),
        "tool[error]>"
    );
}

#[test]
fn terminal_final_answer_has_one_assistant_tag() {
    assert_eq!(
        format_final_answer("finished", true, true),
        "\u{1b}[34massistant>\u{1b}[0m finished"
    );
    assert_eq!(
        format_final_answer("finished", true, false),
        "assistant> finished"
    );
}

#[test]
fn redirected_final_answer_remains_script_friendly() {
    assert_eq!(format_final_answer("finished", false, false), "finished");
}
