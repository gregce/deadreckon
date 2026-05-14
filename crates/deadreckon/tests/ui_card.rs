#![allow(clippy::expect_used)]

use deadreckon::ui_card::{
    Card, CardOptions, HintLine, MetricColumn, Section, TitleGlyph, TitleLine, Tone, pad_visible,
    render_card, strip_ansi, truncate_visible, visible_length,
};

fn known_card() -> Card {
    Card {
        title: TitleLine {
            glyph: TitleGlyph::Preview,
            label: "deadreckon run preview".to_string(),
        },
        subtitle: Some("builds, tests pass, and opens in a browser".to_string()),
        sections: vec![
            Section::Metric {
                label: "turns".to_string(),
                columns: vec![
                    MetricColumn {
                        value: "3 total".to_string(),
                        tone: Tone::Neutral,
                    },
                    MetricColumn {
                        value: "2 done".to_string(),
                        tone: Tone::Good,
                    },
                    MetricColumn {
                        value: "1 waiting".to_string(),
                        tone: Tone::Warn,
                    },
                ],
            },
            Section::KeyValue {
                rows: vec![
                    ("provider".to_string(), "cli:codex".to_string()),
                    (
                        "sleep".to_string(),
                        "caffeinate /usr/bin/caffeinate".to_string(),
                    ),
                ],
            },
            Section::Command {
                label: "show".to_string(),
                command: "deadreckon show abc12345".to_string(),
            },
        ],
        hints: vec![HintLine {
            label: "next".to_string(),
            command: "deadreckon run \"builds\" --yes".to_string(),
        }],
    }
}

#[test]
fn card_renders_fixed_layout_for_known_input() {
    let rendered = render_card(
        &known_card(),
        &CardOptions {
            color: false,
            plain: false,
            terminal_columns: Some(72),
            no_color_env: false,
        },
    );
    let expected = include_str!("fixtures/cards/known-preview.golden");
    assert_eq!(rendered, expected);
}

#[test]
fn card_truncates_with_ellipsis_preserving_active_ansi() {
    let cyan = "\u{1b}[36mhello world\u{1b}[0m";
    let truncated = truncate_visible(cyan, 8);
    assert_eq!(visible_length(&truncated), 8);
    assert!(truncated.ends_with("\u{1b}[0m"), "{truncated:?}");
    assert!(truncated.contains('…'), "{truncated:?}");
}

#[test]
fn card_plain_mode_strips_color_and_box_drawing() {
    let rendered = render_card(
        &known_card(),
        &CardOptions {
            color: true,
            plain: true,
            terminal_columns: Some(72),
            no_color_env: false,
        },
    );
    assert!(!rendered.contains("\u{1b}["), "{rendered:?}");
    assert!(rendered.contains("+"), "{rendered}");
    assert!(rendered.contains("|"), "{rendered}");
    assert!(!rendered.contains("╭"), "{rendered}");
}

#[test]
fn card_visible_length_skips_ansi_escape_sequences() {
    assert_eq!(visible_length("\u{1b}[1;36mdeadreckon\u{1b}[0m"), 10);
    assert_eq!(
        pad_visible("\u{1b}[31mx\u{1b}[0m", 3),
        "\u{1b}[31mx\u{1b}[0m  "
    );
    assert_eq!(strip_ansi("\u{1b}[31mx\u{1b}[0m"), "x");
}

#[test]
fn card_resolves_width_with_terminal_fallback_to_eighty() {
    let mut card = known_card();
    card.subtitle = Some(
        "this deliberately long subtitle proves that None uses an eighty column terminal cap"
            .to_string(),
    );
    let rendered = render_card(
        &card,
        &CardOptions {
            color: false,
            plain: false,
            terminal_columns: None,
            no_color_env: false,
        },
    );
    let first = rendered.lines().next().expect("card top border");
    assert_eq!(visible_length(first), 80);
}

#[test]
fn card_below_forty_cols_falls_back_to_single_column_ascii() {
    let rendered = render_card(
        &known_card(),
        &CardOptions {
            color: true,
            plain: false,
            terminal_columns: Some(32),
            no_color_env: false,
        },
    );
    assert!(!rendered.contains("\u{1b}["), "{rendered:?}");
    assert!(!rendered.contains("╭"), "{rendered}");
    assert!(rendered.contains("deadreckon run preview"), "{rendered}");
}
