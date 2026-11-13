use super::*;
use crate::source::{effective, read};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/alacritty")
}

fn resolve(config: Value) -> Result<Settings, String> {
    Settings::resolve(&config)
}

fn color(settings: &Settings, index: usize) -> &str {
    settings.colors[index].as_deref().unwrap()
}

#[test]
fn defaults_match_alacritty_palette_and_shared_font() {
    let settings = resolve(json!({})).unwrap();
    assert_eq!(settings.colors.len(), COLOR_COUNT);
    for (index, expected) in [
        (0, "#181818"),
        (1, "#ac4242"),
        (15, "#f8f8f8"),
        (258, "#d8d8d8"),
        (259, "#181818"),
        (261, "#0f0f0f"),
        (269, "#8e8e8e"),
    ] {
        assert_eq!(color(&settings, index), expected);
    }
    assert!(
        settings.colors[16].is_none(),
        "unset indexed colors stay synthesized by renderers"
    );
    assert_eq!(
        settings.font_family.as_deref(),
        Some("Atkinson Hyperlegible Mono")
    );
    assert_eq!(
        (
            settings.font_size,
            settings.font_offset_y,
            settings.glyph_offset_y
        ),
        (9.0, -3, 0)
    );
    assert_eq!(
        (settings.history_size, settings.scroll_multiplier),
        (10000, 3)
    );
    assert!(!settings.bold_bright && !settings.update_notifications);
    assert!(settings.group_tabs_by_title, "grouping is on by default");
}

#[test]
fn partial_font_override_keeps_other_shared_defaults() {
    let settings = resolve(json!({"font": {"size": 12}})).unwrap();
    assert_eq!(
        settings.font_family.as_deref(),
        Some("Atkinson Hyperlegible Mono")
    );
    assert_eq!((settings.font_size, settings.font_offset_y), (12.0, -3));
    let settings =
        resolve(json!({"font": {"normal": {"family": "monospace"}, "offset": {"y": 0}}})).unwrap();
    assert_eq!(
        (
            settings.font_family.as_deref(),
            settings.font_offset_y,
            settings.font_size
        ),
        (Some("monospace"), 0, 9.0)
    );
}

#[test]
fn imports_override_by_key_and_missing_imports_are_skipped() {
    let settings = resolve(read(&fixtures().join("main.toml")).unwrap()).unwrap();
    for (index, expected) in [
        (1, "#abcdef"),
        (12, "#aabbcc"),
        (196, "#123456"),
        (256, "#eeeeee"),
        (257, "#454545"),
        (258, "#eeeeee"),
        (259, "#222222"),
        (260, "#fedcba"),
        (261, "#0f0f0f"),
        (263, "#556677"),
        (264, "#a17e4d"),
        (269, "#808080"),
    ] {
        assert_eq!(color(&settings, index), expected, "slot {index}");
    }
    assert_eq!(settings.font_family.as_deref(), Some("Iosevka Term"));
    assert_eq!((settings.history_size, settings.scroll_multiplier), (64, 5));
}

#[test]
fn hex_prefix_and_derived_dim_colors() {
    let settings = resolve(json!({"colors": {"normal": {"red": "0x123456"},
        "primary": {"foreground": "0xabcdef"}}}))
    .unwrap();
    assert_eq!(color(&settings, 1), "#123456");
    assert_eq!(color(&settings, 262), "#0b2238");
    assert_eq!(color(&settings, 269), "#70879d");
}

#[test]
fn unquoted_yaml_hex_colors_are_integers_but_still_colors() {
    let config: Value =
        serde_yaml_ng::from_str("colors:\n  primary:\n    foreground: 0xabcdef\n").unwrap();
    assert_eq!(color(&resolve(config).unwrap(), 258), "#abcdef");
}

#[test]
fn legacy_alacritty_yaml_is_compiled() {
    let settings = resolve(read(&fixtures().join("alacritty.yml")).unwrap()).unwrap();
    assert_eq!(
        settings.font_family.as_deref(),
        Some("Input Mono Condensed")
    );
    assert_eq!(
        (
            settings.font_size,
            settings.font_offset_y,
            settings.glyph_offset_y
        ),
        (6.0, -7, -4)
    );
    assert!(settings.bold_bright);
    assert_eq!(
        (
            color(&settings, 1),
            color(&settings, 258),
            color(&settings, 259)
        ),
        ("#abcdef", "#f9e7c4", "#1d1f21")
    );
}

#[test]
fn worminal_yaml_overrides_alacritty_by_key() {
    let worminal = fixtures().join("worminal.yaml");
    let merged = effective(Some(&fixtures().join("main.toml")), Some(&worminal)).unwrap();
    let settings = resolve(merged).unwrap();
    assert_eq!(settings.font_size, 11.0, "worminal.yaml wins");
    assert_eq!(
        settings.font_family.as_deref(),
        Some("Iosevka Term"),
        "Alacritty fills the rest"
    );
    assert!(settings.update_notifications);
}

#[test]
fn import_cycles_and_bad_types_are_rejected() {
    assert!(read(&fixtures().join("cycle_a.toml"))
        .unwrap_err()
        .contains("cycle"));
    for (config, message) in [
        (json!({"font": {"offset": {"y": 1.5}}}), "font.offset.y"),
        (
            json!({"font": {"glyph_offset": {"y": 1.5}}}),
            "font.glyph_offset.y",
        ),
        (json!({"font": {"size": -1}}), "font.size"),
        (
            json!({"font": {"normal": {"family": ""}}}),
            "font.normal.family",
        ),
        (
            json!({"font": {"normal": {"family": 42}}}),
            "font.normal.family",
        ),
        (
            json!({"scrolling": {"history": 100_001}}),
            "scrolling.history",
        ),
        (
            json!({"scrolling": {"multiplier": 0}}),
            "scrolling.multiplier",
        ),
        (
            json!({"colors": {"normal": {"red": "red; }"}}}),
            "colors.normal.red",
        ),
        (
            json!({"colors": {"indexed_colors": [{"index": 3, "color": "#000000"}]}}),
            "16 through 255",
        ),
        (
            json!({"worminal": {"update_notifications": "yes"}}),
            "worminal.update_notifications",
        ),
        (
            json!({"worminal": {"group_tabs_by_title": 1}}),
            "worminal.group_tabs_by_title",
        ),
    ] {
        let error = resolve(config).unwrap_err();
        assert!(error.contains(message), "{error} should mention {message}");
    }
}

#[test]
fn legacy_bold_setting_at_root_and_notifications_flag() {
    assert!(
        resolve(json!({"draw_bold_text_with_bright_colors": true}))
            .unwrap()
            .bold_bright
    );
    assert!(
        resolve(json!({"worminal": {"update_notifications": true}}))
            .unwrap()
            .update_notifications
    );
    assert!(
        !resolve(json!({"worminal": {"group_tabs_by_title": false}}))
            .unwrap()
            .group_tabs_by_title
    );
}

mod sync_tests {
    use crate::sync::{merge3, parse, render, Prefer};

    fn flat(text: &str) -> crate::sync::Flat {
        parse(text).unwrap()
    }

    #[test]
    fn independent_changes_on_both_sides_combine() {
        let base = flat("font:\n  size: 9\nscrolling:\n  history: 100\n");
        let local = flat("font:\n  size: 12\nscrolling:\n  history: 100\n");
        let remote = flat("font:\n  size: 9\nscrolling:\n  history: 500\n");
        let merged = merge3(&base, &local, &remote, Prefer::Neither);
        assert!(merged.conflicts.is_empty());
        assert_eq!(merged.local, merged.remote);
        assert_eq!(
            render(&merged.local),
            "font:\n  size: 12\nscrolling:\n  history: 500\n"
        );
        assert_eq!(merged.base, merged.local);
    }

    #[test]
    fn same_change_agrees_and_deletions_propagate_unless_other_side_changed() {
        let base = flat("a: 1\nb: 1\nc: 1\n");
        let local = flat("a: 2\nc: 5\n");
        let remote = flat("a: 2\nb: 1\nc: 1\n");
        let merged = merge3(&base, &local, &remote, Prefer::Neither);
        assert!(merged.conflicts.is_empty());
        assert_eq!(
            render(&merged.remote),
            "a: 2\nc: 5\n",
            "b deleted locally, c changed locally"
        );
        let remote = flat("a: 1\nb: 7\nc: 1\n");
        let merged = merge3(&base, &flat("a: 1\nc: 1\n"), &remote, Prefer::Neither);
        assert_eq!(
            merged.conflicts,
            ["b"],
            "deleted on one side, changed on the other"
        );
    }

    #[test]
    fn conflicts_leave_both_sides_and_base_untouched_until_preferred() {
        let base = flat("font:\n  size: 9\nx: 1\n");
        let local = flat("font:\n  size: 10\nx: 2\n");
        let remote = flat("font:\n  size: 11\nx: 1\n");
        let merged = merge3(&base, &local, &remote, Prefer::Neither);
        assert_eq!(merged.conflicts, ["font.size"]);
        assert_eq!(render(&merged.local), "font:\n  size: 10\nx: 2\n");
        assert_eq!(render(&merged.remote), "font:\n  size: 11\nx: 2\n");
        assert_eq!(render(&merged.base), "font:\n  size: 9\nx: 2\n");
        let merged = merge3(&base, &local, &remote, Prefer::Remote);
        assert!(merged.conflicts.is_empty());
        assert_eq!(render(&merged.local), "font:\n  size: 11\nx: 2\n");
    }

    #[test]
    fn first_sync_without_base_conflicts_only_where_values_differ() {
        let merged = merge3(
            &flat(""),
            &flat("a: 1\nb: 2\n"),
            &flat("a: 1\nb: 3\nc: 4\n"),
            Prefer::Neither,
        );
        assert_eq!(merged.conflicts, ["b"]);
        assert_eq!(render(&merged.local), "a: 1\nb: 2\nc: 4\n");
    }

    #[test]
    fn non_mapping_documents_are_rejected() {
        assert!(parse("- 1\n").is_err());
        assert!(parse("").unwrap().is_empty());
    }
}
