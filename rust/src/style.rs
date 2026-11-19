use gpui::{
    App, AssetSource, HighlightStyle, SharedString, StyledText, Svg, prelude::*, px, rgb, svg,
};
use std::borrow::Cow;

pub const UI_FONT: &str = "IBM Plex Sans";
pub const CODE_FONT: &str = "JetBrainsMono Nerd Font Mono";

pub fn register_fonts(cx: &App) -> anyhow::Result<()> {
    cx.text_system().add_fonts(vec![
        Cow::Borrowed(include_bytes!("../resources/fonts/IBMPlexSans-Regular.ttf")),
        Cow::Borrowed(include_bytes!("../resources/fonts/IBMPlexSans-Medium.ttf")),
        Cow::Borrowed(include_bytes!(
            "../resources/fonts/IBMPlexSans-SemiBold.ttf"
        )),
        Cow::Borrowed(include_bytes!("../resources/fonts/IBMPlexSans-Italic.ttf")),
        Cow::Borrowed(include_bytes!(
            "../resources/fonts/JetBrainsMonoNerdFontMono-Regular.ttf"
        )),
    ])
}

// Pure theme data and contrast resolution are shared with the library tests.
pub use agentwatcher::themes::Palette;

pub struct Assets;
impl AssetSource for Assets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        let drawing = match path {
            "folder" => "M3 7V5h6l2 2h10v13H3Z",
            "activity" => "M2 12h5l3-8 4 16 3-8h5",
            "overview" => "M3 11 12 3l9 8M5 10v11h5v-7h4v7h5V10",
            "process" => {
                "M12 3v3m0 12v3M3 12h3m12 0h3M5 5l2 2m10 10 2 2M5 19l2-2M17 7l2-2M8 8h8v8H8Z"
            }
            "increments" => "M4 4h6v6H4ZM14 4h6v6h-6ZM4 14h6v6H4ZM14 14h6v6h-6Z",
            "changes" => "M6 3v12m12-12v6c0 6-12 0-12 6m0 0v6M4 3h4m8 0h4M4 21h4",
            "sessions" => "M4 4h16v13H9l-5 4ZM8 8h8M8 12h6",
            "settings" => {
                "M10 3h4l1 3 3 1 3 3v4l-3 1-1 3-3 3h-4l-1-3-3-1-3-3v-4l3-1 1-3ZM9 12a3 3 0 1 0 6 0 3 3 0 1 0-6 0"
            }
            "search" => "M15 15l6 6M18 10a8 8 0 1 1-16 0 8 8 0 0 1 16 0",
            "left" => "m15 5-7 7 7 7",
            "right" => "m9 5 7 7-7 7",
            "close" => "m6 6 12 12M6 18 18 6",
            "sidebar" => "M3 4h18v16H3ZM9 4v16",
            "dock" => "M3 4h18v16H3ZM15 4v16",
            "refresh" => "M20 7v5h-5M20 12a8 8 0 1 0-2 6M20 12l-5-5",
            "copy" => "M8 8h12v13H8ZM4 16H3V3h12v1",
            "terminal" => "m4 6 5 6-5 6M12 18h8",
            "agent" => "M6 8h12v12H6ZM12 3v5M3 12h3m12 0h3M9 12v2m6-2v2M9 17h6",
            _ => return zed_assets::Assets.load(path),
        };
        Ok(Some(Cow::Owned(format!("<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 24 24\" fill=\"none\" stroke=\"white\" stroke-width=\"1.6\" stroke-linecap=\"round\" stroke-linejoin=\"round\"><path d=\"{drawing}\"/></svg>").into_bytes())))
    }
    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>> {
        zed_assets::Assets.list(path)
    }
}
pub fn icon(name: &'static str, color: u32) -> Svg {
    svg()
        .path(name)
        .size(px(16.))
        .text_color(rgb(color))
        .flex_shrink_0()
}

/// Lightweight lexical coloring for the visible, virtualized source lines.
pub fn source_line(text: &str, p: Palette) -> StyledText {
    static TOKENS: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let tokens = TOKENS.get_or_init(|| regex::Regex::new(
        r#"//[^\n]*|"(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'|\b(?:pub|fn|mod|use|let|mut|struct|enum|impl|return|async|await|const|self|Self|if|else|match|import|from|export|interface|type|function|class|true|false|null|None|Some)\b|\b[A-Z][A-Za-z0-9_]*\b|\b\d+(?:\.\d+)?\b"#
    ).expect("static source token expression"));
    let highlights = tokens
        .find_iter(text)
        .map(|token| {
            let value = token.as_str();
            let color = if value.starts_with("//") {
                p.faint
            } else if value.starts_with(['"', '\'']) {
                p.syntax_string
            } else if value.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                p.syntax_number
            } else if value.chars().next().is_some_and(char::is_uppercase) && value != "Self" {
                p.syntax_type
            } else {
                p.syntax_keyword
            };
            (
                token.range(),
                HighlightStyle {
                    color: Some(rgb(color).into()),
                    ..Default::default()
                },
            )
        })
        .collect::<Vec<_>>();
    StyledText::new(text.to_owned()).with_highlights(highlights)
}
