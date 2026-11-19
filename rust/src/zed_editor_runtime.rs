//! Local runtime for the pinned upstream Zed editor.
//!
//! Buffers are embedded without a Zed workspace, project, client, language
//! server, extension host, or telemetry service. Only the settings, theme,
//! editor registrations, and buffer editing keybindings are initialized here.

use std::sync::Arc;

use gpui::{App, Global, HighlightStyle, Hsla, KeyBinding, TextStyle, px, relative, rgb};
use zed_editor::{EditorStyle, actions::*};
use zed_theme::{ActiveTheme, Appearance, GlobalTheme, LoadThemes, PlayerColor, SyntaxTheme};

use crate::style::{CODE_FONT, Palette, UI_FONT};

struct Initialized;
impl Global for Initialized {}

struct AppliedPalette(Palette);
impl Global for AppliedPalette {}

/// Initialize once, after AgentWatcher's bundled fonts have been registered.
pub fn init(cx: &mut App) {
    if cx.has_global::<Initialized>() {
        return;
    }

    // Match the upstream source version, without reading installed Zed settings
    // or credentials. Release-channel metadata is local and performs no I/O.
    zed_release_channel::init(semver::Version::new(1, 25, 0), cx);
    zed_settings::init(cx);
    let settings = serde_json::json!({
        "ui_font_family": UI_FONT,
        "buffer_font_family": CODE_FONT,
        "buffer_font_size": 14,
        "disable_ai": true,
        "show_edit_predictions": false,
        "enable_language_server": false,
        "language_detection": false,
        "auto_update": false,
        "telemetry": { "diagnostics": false, "metrics": false },
        "minimap": { "show": "never" },
        "show_whitespaces": "none"
    })
    .to_string();
    zed_settings::SettingsStore::update(cx, |store, cx| {
        store
            .set_user_settings(&settings, cx)
            .expect("embedded editor settings must parse");
    });
    zed_theme_settings::init(LoadThemes::JustBase, cx);
    zed_editor::init(cx);
    bind_editor_keys(cx);

    // editor::init also registers NewFile/NewWindow actions intended for Zed's
    // AppState. We deliberately bind neither those nor the full Zed keymap.
    cx.set_global(Initialized);
}

/// Skin the actual upstream theme, including colors read outside EditorStyle.
/// Reapplying an unchanged palette does not trigger global theme notifications.
pub fn apply_palette(p: Palette, cx: &mut App) {
    if cx
        .try_global::<AppliedPalette>()
        .is_some_and(|applied| applied.0 == p)
    {
        return;
    }

    let mut theme = (**cx.theme()).clone();
    theme.id = "agentwatcher-embedded-editor".into();
    theme.name = "AgentWatcher".into();
    theme.appearance = if p.light {
        Appearance::Light
    } else {
        Appearance::Dark
    };
    theme.styles.colors = if p.light {
        zed_theme::ThemeColors::light()
    } else {
        zed_theme::ThemeColors::dark()
    };
    let colors = &mut theme.styles.colors;
    colors.border = color(p.border);
    colors.border_variant = color(p.border);
    colors.border_focused = color(p.focus);
    colors.border_selected = color(p.focus);
    colors.background = color(p.base);
    colors.surface_background = color(p.panel);
    colors.elevated_surface_background = color(p.raised);
    colors.element_background = color(p.control);
    colors.element_hover = color(p.raised);
    colors.element_active = color(p.selected);
    colors.element_selected = color(p.selected);
    colors.element_selection_background = color(p.text_selection);
    colors.ghost_element_hover = color(p.raised);
    colors.ghost_element_active = color(p.selected);
    colors.ghost_element_selected = color(p.selected);
    colors.text = color(p.text);
    colors.text_muted = color(p.muted);
    colors.text_placeholder = color(p.faint);
    colors.text_disabled = color(p.faint);
    colors.text_accent = color(p.accent);
    colors.icon = color(p.text);
    colors.icon_muted = color(p.muted);
    colors.icon_accent = color(p.accent);
    colors.panel_background = color(p.panel);
    colors.toolbar_background = color(p.panel);
    colors.search_match_background = color(p.selected);
    colors.search_active_match_background = color(p.text_selection);
    colors.scrollbar_thumb_background = color(p.border);
    colors.scrollbar_thumb_hover_background = color(p.faint);
    colors.scrollbar_thumb_active_background = color(p.muted);
    colors.scrollbar_thumb_border = color(p.border);
    colors.scrollbar_track_background = color(p.base);
    colors.scrollbar_track_border = color(p.base);
    colors.editor_background = color(p.base);
    colors.editor_foreground = color(p.text);
    colors.editor_gutter_background = color(p.base);
    colors.editor_subheader_background = color(p.panel);
    colors.editor_active_line_background = color(p.panel);
    colors.editor_highlighted_line_background = color(p.selected);
    colors.editor_line_number = color(p.faint);
    colors.editor_active_line_number = color(p.text);
    colors.editor_hover_line_number = color(p.muted);
    colors.editor_invisible = color(p.border);
    colors.editor_wrap_guide = color(p.border);
    colors.editor_active_wrap_guide = color(p.faint);
    colors.editor_indent_guide = color(p.border);
    colors.editor_indent_guide_active = color(p.faint);
    colors.editor_document_highlight_read_background = color(p.selected);
    colors.editor_document_highlight_write_background = color(p.selected);
    colors.editor_document_highlight_bracket_background = color(p.selected);
    colors.editor_diff_hunk_added_background = color(p.added_bg);
    colors.editor_diff_hunk_deleted_background = color(p.removed_bg);

    theme.styles.status = if p.light {
        zed_theme::StatusColors::light()
    } else {
        zed_theme::StatusColors::dark()
    };
    let status = &mut theme.styles.status;
    status.created = color(p.green);
    status.created_background = color(p.added_bg);
    status.created_border = color(p.green);
    status.deleted = color(p.red);
    status.deleted_background = color(p.removed_bg);
    status.deleted_border = color(p.red);
    status.error = color(p.red);
    status.error_background = color(p.removed_bg);
    status.error_border = color(p.red);
    status.success = color(p.green);
    status.success_background = color(p.added_bg);
    status.success_border = color(p.green);
    status.hint = color(p.muted);
    status.hint_background = color(p.panel);
    status.info = color(p.accent);
    status.predictive = color(p.faint);
    status.ignored = color(p.faint);

    theme.styles.player = if p.light {
        zed_theme::PlayerColors::light()
    } else {
        zed_theme::PlayerColors::dark()
    };
    theme.styles.player.0[0] = player(p);
    theme.styles.syntax = syntax(p, theme.styles.syntax);

    GlobalTheme::update_theme(cx, Arc::new(theme));
    cx.set_global(AppliedPalette(p));
}

/// Render through upstream EditorElement with AgentWatcher's font and geometry.
/// Call apply_palette first so gutter, selection and search colors agree too.
pub fn editor_style(p: Palette, font_size: f32, row_height: f32, cx: &App) -> EditorStyle {
    EditorStyle {
        background: color(p.base),
        border: color(p.border),
        local_player: player(p),
        text: TextStyle {
            color: color(p.text),
            font_family: CODE_FONT.into(),
            font_size: px(font_size).into(),
            line_height: relative(row_height / font_size),
            ..Default::default()
        },
        // Upstream ui::EDITOR_SCROLLBAR_WIDTH at the pinned revision; the
        // matching EditorElement constant is crate-private.
        scrollbar_width: px(15.),
        syntax: cx.theme().syntax().clone(),
        status: cx.theme().status().clone(),
        inlay_hints_style: zed_editor::make_inlay_hints_style(cx),
        edit_prediction_styles: zed_editor::make_suggestion_styles(cx),
        unnecessary_code_fade: 0.5,
        show_underlines: true,
    }
}

fn color(value: u32) -> Hsla {
    rgb(value).into()
}

fn player(p: Palette) -> PlayerColor {
    PlayerColor {
        cursor: color(p.focus),
        background: color(p.selected),
        selection: color(p.text_selection),
    }
}

fn syntax(p: Palette, base: Arc<SyntaxTheme>) -> Arc<SyntaxTheme> {
    SyntaxTheme::merge(
        base,
        [
            ("attribute", p.syntax_type),
            ("boolean", p.syntax_number),
            ("comment", p.faint),
            ("comment.doc", p.muted),
            ("constant", p.syntax_number),
            ("constructor", p.syntax_type),
            ("diff.plus", p.green),
            ("diff.minus", p.red),
            ("enum", p.syntax_type),
            ("function", p.syntax_type),
            ("function.method", p.syntax_type),
            ("function.definition", p.syntax_type),
            ("hint", p.muted),
            ("keyword", p.syntax_keyword),
            ("label", p.syntax_keyword),
            ("link_text", p.accent),
            ("link_uri", p.accent),
            ("number", p.syntax_number),
            ("operator", p.text),
            ("preproc", p.syntax_keyword),
            ("property", p.text),
            ("punctuation", p.text),
            ("string", p.syntax_string),
            ("string.escape", p.syntax_keyword),
            ("string.regex", p.syntax_string),
            ("tag", p.syntax_keyword),
            ("text.literal", p.syntax_string),
            ("type", p.syntax_type),
            ("variable", p.text),
            ("variable.special", p.syntax_keyword),
        ]
        .into_iter()
        .map(|(name, value)| {
            (
                name.to_owned(),
                HighlightStyle {
                    color: Some(color(value)),
                    ..Default::default()
                },
            )
        })
        .collect(),
    )
}

fn bind_editor_keys(cx: &mut App) {
    let context = Some("Editor");
    // Buffer-local actions only. AgentWatcher's CodeView owns find and zoom;
    // workspace, LSP, collaboration, AI, and shell actions are not bound here.
    cx.bind_keys([
        KeyBinding::new("cmd-c", Copy, context),
        KeyBinding::new("cmd-x", Cut, context),
        KeyBinding::new("cmd-v", Paste, context),
        KeyBinding::new("cmd-a", SelectAll, context),
        KeyBinding::new("cmd-z", Undo, context),
        KeyBinding::new("cmd-shift-z", Redo, context),
        KeyBinding::new("backspace", Backspace, context),
        KeyBinding::new("shift-backspace", Backspace, context),
        KeyBinding::new("delete", Delete, context),
        KeyBinding::new("ctrl-h", Backspace, context),
        KeyBinding::new("ctrl-d", Delete, context),
        KeyBinding::new(
            "alt-backspace",
            DeleteToPreviousWordStart::default(),
            context,
        ),
        KeyBinding::new("ctrl-w", DeleteToPreviousWordStart::default(), context),
        KeyBinding::new("alt-delete", DeleteToNextWordEnd::default(), context),
        KeyBinding::new("cmd-backspace", DeleteToBeginningOfLine::default(), context),
        KeyBinding::new("cmd-delete", DeleteToEndOfLine, context),
        KeyBinding::new("tab", Tab, context),
        KeyBinding::new("shift-tab", Backtab, context),
        KeyBinding::new("enter", Newline, Some("Editor && mode == full")),
        KeyBinding::new("left", MoveLeft, context),
        KeyBinding::new("right", MoveRight, context),
        KeyBinding::new("up", zed_actions::editor::MoveUp, context),
        KeyBinding::new("down", zed_actions::editor::MoveDown, context),
        KeyBinding::new("shift-left", SelectLeft, context),
        KeyBinding::new("shift-right", SelectRight, context),
        KeyBinding::new("shift-up", SelectUp, context),
        KeyBinding::new("shift-down", SelectDown, context),
        KeyBinding::new("alt-left", MoveToPreviousWordStart, context),
        KeyBinding::new("alt-right", MoveToNextWordEnd, context),
        KeyBinding::new("alt-shift-left", SelectToPreviousWordStart, context),
        KeyBinding::new("alt-shift-right", SelectToNextWordEnd, context),
        KeyBinding::new("home", MoveToBeginningOfLine::default(), context),
        KeyBinding::new("end", MoveToEndOfLine::default(), context),
        KeyBinding::new("cmd-left", MoveToBeginningOfLine::default(), context),
        KeyBinding::new("cmd-right", MoveToEndOfLine::default(), context),
        KeyBinding::new("ctrl-a", MoveToBeginningOfLine::default(), context),
        KeyBinding::new("ctrl-e", MoveToEndOfLine::default(), context),
        KeyBinding::new("shift-home", SelectToBeginningOfLine::default(), context),
        KeyBinding::new("shift-end", SelectToEndOfLine::default(), context),
        KeyBinding::new(
            "cmd-shift-left",
            SelectToBeginningOfLine::default(),
            context,
        ),
        KeyBinding::new("cmd-shift-right", SelectToEndOfLine::default(), context),
        KeyBinding::new("cmd-up", MoveToBeginning, context),
        KeyBinding::new("cmd-down", MoveToEnd, context),
        KeyBinding::new("cmd-home", MoveToBeginning, context),
        KeyBinding::new("cmd-end", MoveToEnd, context),
        KeyBinding::new("cmd-shift-up", SelectToBeginning, context),
        KeyBinding::new("cmd-shift-down", SelectToEnd, context),
        KeyBinding::new("pageup", MovePageUp::default(), context),
        KeyBinding::new("pagedown", MovePageDown::default(), context),
        KeyBinding::new("shift-pageup", SelectPageUp, context),
        KeyBinding::new("shift-pagedown", SelectPageDown, context),
    ]);
}
