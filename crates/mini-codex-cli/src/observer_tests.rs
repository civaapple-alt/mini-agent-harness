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
