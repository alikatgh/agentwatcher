//! Bundled offline themes inspired by Earendil's system-theme article.
//!
//! Only the source foreground, background and 16 ANSI colors are retained from
//! the demo. AgentWatcher's semantic colors use its own WCAG contrast solver;
//! none of Pi's computed tokens, fitted coefficients or implementation is used.
//! See resources/themes/ATTRIBUTION.md for palette provenance.

use std::sync::OnceLock;

/// Stable persisted identifier, presentation metadata and original theme colors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThemePreset {
    pub id: &'static str,
    pub name: &'static str,
    pub light: bool,
    pub background: u32,
    pub foreground: u32,
    /// Standard ANSI order: black..white, followed by bright black..white.
    pub ansi: [u32; 16],
    /// Decorative preview: canvas, accent, success, error, primary foreground.
    pub swatches: [u32; 5],
}

const fn preset(
    id: &'static str,
    name: &'static str,
    light: bool,
    background: u32,
    foreground: u32,
    ansi: [u32; 16],
) -> ThemePreset {
    ThemePreset {
        id,
        name,
        light,
        background,
        foreground,
        ansi,
        swatches: [background, ansi[5], ansi[2], ansi[1], foreground],
    }
}

static PRESETS: [ThemePreset; 25] = [
    preset(
        "rose-pine",
        "Rosé Pine",
        false,
        0x191724,
        0xe0def4,
        [
            0x26233a, 0xeb6f92, 0x31748f, 0xf6c177, 0x9ccfd8, 0xc4a7e7, 0xebbcba, 0xe0def4,
            0x6e6a86, 0xeb6f92, 0x31748f, 0xf6c177, 0x9ccfd8, 0xc4a7e7, 0xebbcba, 0xe0def4,
        ],
    ),
    preset(
        "tokyo-night",
        "Tokyo Night",
        false,
        0x1a1b26,
        0xc0caf5,
        [
            0x15161e, 0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xa9b1d6,
            0x414868, 0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xc0caf5,
        ],
    ),
    preset(
        "solarized-dark",
        "Solarized Dark",
        false,
        0x002b36,
        0x839496,
        [
            0x073642, 0xdc322f, 0x859900, 0xb58900, 0x268bd2, 0xd33682, 0x2aa198, 0xeee8d5,
            0x335e69, 0xcb4b16, 0x586e75, 0x657b83, 0x839496, 0x6c71c4, 0x93a1a1, 0xfdf6e3,
        ],
    ),
    preset(
        "one-dark",
        "One Dark",
        false,
        0x21252b,
        0xabb2bf,
        [
            0x21252b, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xabb2bf,
            0x767676, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xabb2bf,
        ],
    ),
    preset(
        "material-dark",
        "Material Dark",
        false,
        0x232322,
        0xe5e5e5,
        [
            0x212121, 0xb7141f, 0x457b24, 0xf6981e, 0x134eb2, 0x701aa2, 0x0e717c, 0xefefef,
            0x4f4f4f, 0xe83b3f, 0x7aba3a, 0xffea2e, 0x54a4f3, 0xaa4dbc, 0x26bbd1, 0xd9d9d9,
        ],
    ),
    preset(
        "gruvbox-dark",
        "Gruvbox Dark",
        false,
        0x282828,
        0xebdbb2,
        [
            0x282828, 0xcc241d, 0x98971a, 0xd79921, 0x458588, 0xb16286, 0x689d6a, 0xa89984,
            0x928374, 0xfb4934, 0xb8bb26, 0xfabd2f, 0x83a598, 0xd3869b, 0x8ec07c, 0xebdbb2,
        ],
    ),
    preset(
        "dracula",
        "Dracula",
        false,
        0x282a36,
        0xf8f8f2,
        [
            0x21222c, 0xff5555, 0x50fa7b, 0xf1fa8c, 0xbd93f9, 0xff79c6, 0x8be9fd, 0xf8f8f2,
            0x6272a4, 0xff6e6e, 0x69ff94, 0xffffa5, 0xd6acff, 0xff92df, 0xa4ffff, 0xffffff,
        ],
    ),
    preset(
        "ghostty-default",
        "Ghostty Default",
        false,
        0x282c34,
        0xffffff,
        [
            0x1d1f21, 0xcc6666, 0xb5bd68, 0xf0c674, 0x81a2be, 0xb294bb, 0x8abeb7, 0xc5c8c6,
            0x666666, 0xd54e53, 0xb9ca4a, 0xe7c547, 0x7aa6da, 0xc397d8, 0x70c0b1, 0xeaeaea,
        ],
    ),
    preset(
        "nord",
        "Nord",
        false,
        0x2e3440,
        0xd8dee9,
        [
            0x3b4252, 0xbf616a, 0xa3be8c, 0xebcb8b, 0x81a1c1, 0xb48ead, 0x88c0d0, 0xe5e9f0,
            0x596377, 0xbf616a, 0xa3be8c, 0xebcb8b, 0x81a1c1, 0xb48ead, 0x8fbcbb, 0xeceff4,
        ],
    ),
    preset(
        "catppuccin-frappe",
        "Catppuccin Frappé",
        false,
        0x303446,
        0xc6d0f5,
        [
            0x51576d, 0xe78284, 0xa6d189, 0xe5c890, 0x8caaee, 0xf4b8e4, 0x81c8be, 0xa5adce,
            0x626880, 0xe67172, 0x8ec772, 0xd9ba73, 0x7b9ef0, 0xf2a4db, 0x5abfb5, 0xb5bfe2,
        ],
    ),
    preset(
        "catppuccin-mocha",
        "Catppuccin Mocha",
        false,
        0x1e1e2e,
        0xcdd6f4,
        [
            0x45475a, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xa6adc8,
            0x585b70, 0xf37799, 0x89d88b, 0xebd391, 0x74a8fc, 0xf2aede, 0x6bd7ca, 0xbac2de,
        ],
    ),
    preset(
        "palenight",
        "Palenight",
        false,
        0x3e4251,
        0xcccccc,
        [
            0x000000, 0xf07178, 0xc3e88d, 0xffcb6b, 0x82aaff, 0xc792ea, 0x89ddff, 0xffffff,
            0x737373, 0xf6a9ae, 0xdbf1ba, 0xffdfa6, 0xb4ccff, 0xddbdf2, 0xb8eaff, 0x999999,
        ],
    ),
    preset(
        "ocean",
        "Ocean",
        false,
        0x224fbc,
        0xffffff,
        [
            0x000000, 0xe64c4c, 0x00a600, 0x999900, 0x0000b2, 0xd826d8, 0x00a6b2, 0xbfbfbf,
            0x808080, 0xff1a1a, 0x00d900, 0xe5e500, 0x7373ff, 0xe500e5, 0x00e5e5, 0xe5e5e5,
        ],
    ),
    preset(
        "tokyo-night-day",
        "Tokyo Night Day",
        true,
        0xe1e2e7,
        0x3760bf,
        [
            0xe9e9ed, 0xf52a65, 0x587539, 0x8c6c3e, 0x2e7de9, 0x9854f1, 0x007197, 0x6172b0,
            0xa1a6c5, 0xf52a65, 0x587539, 0x8c6c3e, 0x2e7de9, 0x9854f1, 0x007197, 0x3760bf,
        ],
    ),
    preset(
        "nord-light",
        "Nord Light",
        true,
        0xe5e9f0,
        0x414858,
        [
            0x3b4252, 0xbf616a, 0x96b17f, 0xc5a565, 0x81a1c1, 0xb48ead, 0x7bb3c3, 0xa5abb6,
            0x4c566a, 0xbf616a, 0x96b17f, 0xc5a565, 0x81a1c1, 0xb48ead, 0x82afae, 0xeceff4,
        ],
    ),
    preset(
        "catppuccin-latte",
        "Catppuccin Latte",
        true,
        0xeff1f5,
        0x4c4f69,
        [
            0x5c5f77, 0xd20f39, 0x40a02b, 0xdf8e1d, 0x1e66f5, 0xea76cb, 0x179299, 0xacb0be,
            0x6c6f85, 0xde293e, 0x49af3d, 0xeea02d, 0x456eff, 0xfe85d8, 0x2d9fa8, 0xbcc0cc,
        ],
    ),
    preset(
        "ayu-light",
        "Ayu Light",
        true,
        0xf8f9fa,
        0x5c6166,
        [
            0x000000, 0xea6c6d, 0x6cbf43, 0xeca944, 0x3199e1, 0x9e75c7, 0x46ba94, 0xbababa,
            0x686868, 0xf07171, 0x86b300, 0xf2ae49, 0x399ee6, 0xa37acc, 0x4cbf99, 0xd1d1d1,
        ],
    ),
    preset(
        "one-light",
        "One Light",
        true,
        0xf9f9f9,
        0x2a2c33,
        [
            0x000000, 0xde3e35, 0x3f953a, 0xd2b67c, 0x2f5af3, 0x950095, 0x3f953a, 0xbbbbbb,
            0x000000, 0xde3e35, 0x3f953a, 0xd2b67c, 0x2f5af3, 0xa00095, 0x3f953a, 0xffffff,
        ],
    ),
    preset(
        "github-light",
        "GitHub Light",
        true,
        0xffffff,
        0x1f2328,
        [
            0x24292f, 0xcf222e, 0x116329, 0x4d2d00, 0x0969da, 0x8250df, 0x1b7c83, 0x6e7781,
            0x57606a, 0xa40e26, 0x1a7f37, 0x633c01, 0x218bff, 0xa475f9, 0x3192aa, 0x8c959f,
        ],
    ),
    preset(
        "monokai-pro-light",
        "Monokai Pro Light",
        true,
        0xfaf4f2,
        0x29242a,
        [
            0xfaf4f2, 0xe14775, 0x269d69, 0xcc7a0a, 0xe16032, 0x7058be, 0x1c8ca8, 0x29242a,
            0xa59fa0, 0xe14775, 0x269d69, 0xcc7a0a, 0xe16032, 0x7058be, 0x1c8ca8, 0x29242a,
        ],
    ),
    preset(
        "rose-pine-dawn",
        "Rosé Pine Dawn",
        true,
        0xfaf4ed,
        0x575279,
        [
            0xf2e9e1, 0xb4637a, 0x286983, 0xea9d34, 0x56949f, 0x907aa9, 0xd7827e, 0x575279,
            0x9893a5, 0xb4637a, 0x286983, 0xea9d34, 0x56949f, 0x907aa9, 0xd7827e, 0x575279,
        ],
    ),
    preset(
        "solarized-light",
        "Solarized Light",
        true,
        0xfdf6e3,
        0x657b83,
        [
            0x073642, 0xdc322f, 0x859900, 0xb58900, 0x268bd2, 0xd33682, 0x2aa198, 0xbbb5a2,
            0x002b36, 0xcb4b16, 0x586e75, 0x657b83, 0x839496, 0x6c71c4, 0x93a1a1, 0xfdf6e3,
        ],
    ),
    preset(
        "gruvbox-light",
        "Gruvbox Light",
        true,
        0xfbf1c7,
        0x3c3836,
        [
            0xfbf1c7, 0xcc241d, 0x98971a, 0xd79921, 0x458588, 0xb16286, 0x689d6a, 0x7c6f64,
            0x928374, 0x9d0006, 0x79740e, 0xb57614, 0x076678, 0x8f3f71, 0x427b58, 0x3c3836,
        ],
    ),
    preset(
        "kanagawa-lotus",
        "Kanagawa Lotus",
        true,
        0xf2ecbc,
        0x545464,
        [
            0x1f1f28, 0xc84053, 0x6f894e, 0x77713f, 0x4d699b, 0xb35b79, 0x597b75, 0x545464,
            0x8a8980, 0xd7474b, 0x6e915f, 0x836f4a, 0x6693bf, 0x624c83, 0x5e857a, 0x43436c,
        ],
    ),
    preset(
        "everforest-light",
        "Everforest Light",
        true,
        0xefebd4,
        0x5c6a72,
        [
            0x7a8478, 0xe67e80, 0x9ab373, 0xc1a266, 0x7fbbb3, 0xd699b6, 0x83c092, 0xb2af9f,
            0xa6b0a0, 0xf85552, 0x8da101, 0xdfa000, 0x3a94c5, 0xdf69ba, 0x35a77c, 0xfffbef,
        ],
    ),
];

/// All named themes, in the article's dark/light order, with Mocha beside Frappé.
pub fn presets() -> &'static [ThemePreset] {
    &PRESETS
}

pub fn find(id: &str) -> Option<&'static ThemePreset> {
    PRESETS.iter().find(|theme| theme.id == id)
}

/// Accept the existing adaptive/default appearances as well as named presets.
pub fn valid_appearance(id: &str) -> bool {
    matches!(id, "system" | "light" | "dark") || find(id).is_some()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    pub light: bool,
    pub base: u32,
    pub panel: u32,
    pub raised: u32,
    pub control: u32,
    pub selected: u32,
    pub focus: u32,
    pub text_selection: u32,
    pub inactive_selection: u32,
    pub added_bg: u32,
    pub removed_bg: u32,
    /// Quiet pane and row separators; not an input or selection indicator.
    pub border: u32,
    /// Required field boundaries, resolved against every workspace plane.
    pub control_border: u32,
    pub text: u32,
    pub muted: u32,
    pub faint: u32,
    pub accent: u32,
    pub green: u32,
    pub red: u32,
    pub syntax_keyword: u32,
    pub syntax_string: u32,
    pub syntax_number: u32,
    pub syntax_type: u32,
}
impl Palette {
    /// Resolve a persisted appearance. Named presets ignore system appearance;
    /// unknown values safely follow it, just like the adaptive `system` default.
    /// All named palettes are computed once and copied from the immutable cache.
    pub fn for_theme(appearance: &str, system_light: bool) -> Self {
        match appearance {
            "light" => Self::new(true),
            "dark" => Self::new(false),
            _ => PRESETS
                .iter()
                .position(|theme| theme.id == appearance)
                .map(|index| resolved_presets()[index])
                .unwrap_or_else(|| Self::new(system_light)),
        }
    }

    pub fn new(light: bool) -> Self {
        if light {
            Self {
                light: true,
                base: 0xfafafa,
                panel: 0xf0f0f0,
                raised: 0xffffff,
                control: 0xe6e6e6,
                selected: 0xd5e3f7,
                focus: 0x2059a8,
                text_selection: 0xc7dcf8,
                inactive_selection: 0xe3e3e3,
                added_bg: 0xe7f0e7,
                removed_bg: 0xf7e9e9,
                border: 0xd3d3d3,
                control_border: 0x767a80,
                text: 0x24272d,
                muted: 0x494e56,
                faint: 0x555a62,
                accent: 0x2059a8,
                green: 0x2f613c,
                red: 0xa1383d,
                syntax_keyword: 0x773c99,
                syntax_string: 0x446522,
                syntax_number: 0x875018,
                syntax_type: 0x005f73,
            }
        } else {
            Self {
                light: false,
                base: 0x1f1f1f,
                panel: 0x181818,
                raised: 0x282828,
                control: 0x2d2d2d,
                selected: 0x223a53,
                focus: 0x83b5ff,
                text_selection: 0x213c59,
                inactive_selection: 0x333333,
                added_bg: 0x203226,
                removed_bg: 0x392829,
                border: 0x404040,
                control_border: 0x858990,
                text: 0xe6e6e6,
                muted: 0xb8b8b8,
                faint: 0xa8a8a8,
                accent: 0x83b5ff,
                green: 0x9dca9b,
                red: 0xf09c98,
                syntax_keyword: 0xdb9ad5,
                syntax_string: 0xb1cb8e,
                syntax_number: 0xe0b48c,
                syntax_type: 0x8cc9ec,
            }
        }
    }
}

fn resolved_presets() -> &'static [Palette; 25] {
    static PALETTES: OnceLock<[Palette; 25]> = OnceLock::new();
    PALETTES.get_or_init(|| PRESETS.map(Palette::from_preset))
}

impl Palette {
    fn from_preset(theme: ThemePreset) -> Self {
        let base = theme.background;
        let base_lab = Lab::from_rgb(base);
        let toward_text = if theme.light { -1. } else { 1. };
        let plane = |offset| base_lab.with_lightness(base_lab.l + offset).to_rgb();
        let tinted_plane = |seed, offset, chroma| {
            Lab::from_rgb(seed)
                .with_lightness(base_lab.l + offset)
                .with_max_chroma(chroma)
                .to_rgb()
        };

        let panel = plane(-0.025);
        let raised = plane(if theme.light { 0.020 } else { 0.035 });
        let control = plane(toward_text * 0.055);
        let selected = tinted_plane(theme.ansi[5], toward_text * 0.055, 0.040);
        let text_selection = tinted_plane(theme.ansi[4], toward_text * 0.065, 0.050);
        let inactive_selection = plane(toward_text * 0.045);
        let added_bg = tinted_plane(theme.ansi[2], toward_text * 0.035, 0.035);
        let removed_bg = tinted_plane(theme.ansi[1], toward_text * 0.035, 0.035);
        let backgrounds = [
            base,
            panel,
            raised,
            control,
            selected,
            text_selection,
            inactive_selection,
            added_bg,
            removed_bg,
        ];
        let readable = |seed, target| contrast_color(seed, &backgrounds, target, theme.light);

        let text = readable(theme.foreground, 7.0);
        let muted = readable(mix(text, base, 0.30), 5.25);
        let faint = readable(mix(text, base, 0.45), 4.55);
        let accent = readable(theme.ansi[5], 4.55);
        Self {
            light: theme.light,
            base,
            panel,
            raised,
            control,
            selected,
            focus: readable(theme.ansi[5], 3.05),
            text_selection,
            inactive_selection,
            added_bg,
            removed_bg,
            // This role is the app's decorative pane/row separator. Required
            // interaction indicators use control_border/focus/accent instead.
            border: mix(base, text, 0.20),
            control_border: readable(mix(base, text, 0.20), 3.05),
            text,
            muted,
            faint,
            accent,
            green: readable(theme.ansi[2], 4.55),
            red: readable(theme.ansi[1], 4.55),
            syntax_keyword: readable(theme.ansi[5], 4.55),
            syntax_string: readable(theme.ansi[2], 4.55),
            syntax_number: readable(theme.ansi[3], 4.55),
            syntax_type: readable(theme.ansi[6], 4.55),
        }
    }
}

fn luminance(color: u32) -> f64 {
    let [r, g, b] = linear_rgb(color);
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

fn contrast(foreground: u32, background: u32) -> f64 {
    let a = luminance(foreground);
    let b = luminance(background);
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

fn minimum_contrast(color: u32, backgrounds: &[u32]) -> f64 {
    backgrounds
        .iter()
        .map(|background| contrast(color, *background))
        .fold(f64::INFINITY, f64::min)
}

/// Move lightness toward the readable end of the scale, keeping source hue and
/// never increasing its chroma. The check uses the final quantized sRGB color,
/// so gamut mapping and 8-bit rounding cannot silently invalidate the result.
fn contrast_color(source: u32, backgrounds: &[u32], target: f64, light: bool) -> u32 {
    if minimum_contrast(source, backgrounds) >= target {
        return source;
    }
    let endpoint = if light { 0x000000 } else { 0xffffff };
    // A saturated mid-dark canvas such as Ocean cannot support 7:1 everywhere;
    // give primary text the strongest available contrast on that exact canvas.
    let target = target.min(minimum_contrast(endpoint, backgrounds));
    let original = Lab::from_rgb(source);
    let mut failing = original.l;
    let mut passing = if light { 0. } else { 1. };
    let mut best = endpoint;
    for _ in 0..24 {
        let candidate_l = (failing + passing) * 0.5;
        let candidate = original.with_lightness(candidate_l).to_rgb();
        if minimum_contrast(candidate, backgrounds) >= target {
            passing = candidate_l;
            best = candidate;
        } else {
            failing = candidate_l;
        }
    }
    best
}

fn linear_rgb(color: u32) -> [f64; 3] {
    [16, 8, 0].map(|shift| {
        let value = ((color >> shift) & 255) as f64 / 255.;
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    })
}

/// Standard OKLab coordinates (Björn Ottosson's published color-space matrices).
/// The gamut fit and contrast search here are independent AgentWatcher code.
#[derive(Clone, Copy)]
struct Lab {
    l: f64,
    a: f64,
    b: f64,
}

impl Lab {
    fn from_rgb(color: u32) -> Self {
        let [r, g, b] = linear_rgb(color);
        let l = (0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b).cbrt();
        let m = (0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b).cbrt();
        let s = (0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b).cbrt();
        Self {
            l: 0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s,
            a: 1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s,
            b: 0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s,
        }
    }

    fn with_lightness(self, l: f64) -> Self {
        Self {
            l: l.clamp(0., 1.),
            ..self
        }
    }

    fn with_max_chroma(self, max_chroma: f64) -> Self {
        let chroma = self.a.hypot(self.b);
        let scale = if chroma > max_chroma {
            max_chroma / chroma
        } else {
            1.
        };
        Self {
            a: self.a * scale,
            b: self.b * scale,
            ..self
        }
    }

    fn linear_rgb(self, chroma_scale: f64) -> [f64; 3] {
        let a = self.a * chroma_scale;
        let b = self.b * chroma_scale;
        let l = (self.l + 0.3963377774 * a + 0.2158037573 * b).powi(3);
        let m = (self.l - 0.1055613458 * a - 0.0638541728 * b).powi(3);
        let s = (self.l - 0.0894841775 * a - 1.2914855480 * b).powi(3);
        [
            4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
            -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
            -0.0041960863 * l - 0.7034186147 * m + 1.7076147010 * s,
        ]
    }

    fn to_rgb(self) -> u32 {
        if self.l <= 0. {
            return 0x000000;
        }
        if self.l >= 1. {
            return 0xffffff;
        }
        let in_gamut = |rgb: [f64; 3]| rgb.iter().all(|channel| (0.0..=1.0).contains(channel));
        let mut rgb = self.linear_rgb(1.);
        if !in_gamut(rgb) {
            let mut low = 0.;
            let mut high = 1.;
            for _ in 0..20 {
                let scale = (low + high) * 0.5;
                if in_gamut(self.linear_rgb(scale)) {
                    low = scale;
                } else {
                    high = scale;
                }
            }
            rgb = self.linear_rgb(low);
        }
        rgb.into_iter().fold(0, |packed, channel| {
            let channel = channel.clamp(0., 1.);
            let encoded = if channel <= 0.0031308 {
                12.92 * channel
            } else {
                1.055 * channel.powf(1. / 2.4) - 0.055
            };
            (packed << 8) | (encoded * 255.).round() as u32
        })
    }
}

fn mix(first: u32, second: u32, second_weight: f64) -> u32 {
    let a = Lab::from_rgb(first);
    let b = Lab::from_rgb(second);
    let first_weight = 1. - second_weight;
    Lab {
        l: a.l * first_weight + b.l * second_weight,
        a: a.a * first_weight + b.a * second_weight,
        b: a.b * first_weight + b.b * second_weight,
    }
    .to_rgb()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn backgrounds(p: Palette) -> [(&'static str, u32); 9] {
        [
            ("canvas", p.base),
            ("panel", p.panel),
            ("raised surface", p.raised),
            ("control", p.control),
            ("selected row", p.selected),
            ("added source line", p.added_bg),
            ("removed source line", p.removed_bg),
            ("selected source text", p.text_selection),
            ("inactive selection", p.inactive_selection),
        ]
    }

    #[test]
    fn complete_catalog_has_stable_unique_names_and_ids() {
        assert_eq!(presets().len(), 25);
        assert_eq!(presets().iter().filter(|theme| theme.light).count(), 12);
        let mut ids = HashSet::new();
        let mut names = HashSet::new();
        for theme in presets() {
            assert!(ids.insert(theme.id), "duplicate ID: {}", theme.id);
            assert!(names.insert(theme.name), "duplicate name: {}", theme.name);
            assert!(theme.id.chars().all(|c| c.is_ascii_lowercase() || c == '-'));
            assert_eq!(find(theme.id), Some(theme));
            assert!(valid_appearance(theme.id));
            assert_eq!(theme.swatches[0], theme.background);
        }
        for id in ["system", "light", "dark"] {
            assert!(valid_appearance(id));
        }
        assert_eq!(find("rose-pine").unwrap().name, "Rosé Pine");
        assert_eq!(find("catppuccin-frappe").unwrap().name, "Catppuccin Frappé");
        assert_eq!(find("catppuccin-mocha").unwrap().background, 0x1e1e2e);
        assert_eq!(find("catppuccin-mocha").unwrap().foreground, 0xcdd6f4);
        assert!(!valid_appearance("unknown-theme"));
        assert!(find("system").is_none());
    }

    #[test]
    fn named_themes_keep_the_original_canvas_and_ignore_system_appearance() {
        for theme in presets() {
            let p = Palette::for_theme(theme.id, false);
            assert_eq!(p.base, theme.background, "{} changed canvas", theme.name);
            assert_eq!(p.light, theme.light);
            assert_eq!(p, Palette::for_theme(theme.id, true));
            assert_ne!(p.base, p.panel, "{} lost its pane hierarchy", theme.name);
            // Pure white canvases cannot become lighter; their raised plane is
            // delineated by the existing pane separator rather than inversion.
            if theme.background != 0xffffff {
                assert_ne!(p.base, p.raised, "{} lost its raised plane", theme.name);
            }
            assert_ne!(p.base, p.selected, "{} lost selection", theme.name);
        }
    }

    #[test]
    fn default_and_invalid_appearances_preserve_existing_resolution() {
        for system_light in [false, true] {
            assert_eq!(
                Palette::for_theme("system", system_light),
                Palette::new(system_light)
            );
            assert_eq!(
                Palette::for_theme("unknown-theme", system_light),
                Palette::new(system_light)
            );
            assert_eq!(
                Palette::for_theme("light", system_light),
                Palette::new(true)
            );
            assert_eq!(
                Palette::for_theme("dark", system_light),
                Palette::new(false)
            );
        }
    }

    #[test]
    fn resolved_palette_cache_is_reused_for_all_lookups() {
        let cached = resolved_presets();
        for _ in 0..8 {
            assert!(std::ptr::eq(cached, resolved_presets()));
            for (index, theme) in presets().iter().enumerate() {
                assert_eq!(Palette::for_theme(theme.id, true), cached[index]);
            }
        }
    }

    #[test]
    fn all_normal_text_syntax_and_status_colors_are_readable_on_every_background() {
        assert!((contrast(0x000000, 0xffffff) - 21.).abs() < 0.001);
        for (name, p) in presets()
            .iter()
            .map(|theme| (theme.name, Palette::for_theme(theme.id, false)))
            .chain([
                ("Default Light", Palette::new(true)),
                ("Default Dark", Palette::new(false)),
            ])
        {
            for (role, foreground) in [
                ("primary text", p.text),
                ("secondary text", p.muted),
                ("required metadata", p.faint),
                ("action text", p.accent),
                ("added text", p.green),
                ("removed text", p.red),
                ("syntax keyword", p.syntax_keyword),
                ("syntax string", p.syntax_string),
                ("syntax number", p.syntax_number),
                ("syntax type", p.syntax_type),
            ] {
                for (surface, background) in backgrounds(p) {
                    let ratio = contrast(foreground, background);
                    assert!(
                        ratio >= 4.5,
                        "{name}: {role} #{foreground:06x} on {surface} #{background:06x} is {ratio:.3}:1"
                    );
                }
            }
        }
    }

    #[test]
    fn focus_indicator_is_distinguishable_from_every_workspace_plane() {
        for (name, p) in presets()
            .iter()
            .map(|theme| (theme.name, Palette::for_theme(theme.id, false)))
            .chain([
                ("Default Light", Palette::new(true)),
                ("Default Dark", Palette::new(false)),
            ])
        {
            for (surface, background) in backgrounds(p) {
                let ratio = contrast(p.focus, background);
                assert!(ratio >= 3., "{name}: focus on {surface} is {ratio:.3}:1");
            }
        }
    }

    #[test]
    fn required_control_boundaries_are_distinct_from_decorative_separators() {
        for (name, p) in presets()
            .iter()
            .map(|theme| (theme.name, Palette::for_theme(theme.id, false)))
            .chain([
                ("Default Light", Palette::new(true)),
                ("Default Dark", Palette::new(false)),
            ])
        {
            assert_ne!(p.control_border, p.border, "{name}: boundary roles merged");
            for (surface, background) in backgrounds(p) {
                let ratio = contrast(p.control_border, background);
                assert!(
                    ratio >= 3.,
                    "{name}: control boundary #{:06x} on {surface} #{background:06x} is {ratio:.3}:1",
                    p.control_border,
                );
            }
        }
    }

    #[test]
    fn adjusted_foregrounds_preserve_hue_and_do_not_gain_chroma() {
        for theme in presets() {
            let p = Palette::for_theme(theme.id, false);
            for (source, resolved) in [
                (theme.ansi[5], p.accent),
                (theme.ansi[2], p.green),
                (theme.ansi[1], p.red),
                (theme.ansi[5], p.syntax_keyword),
                (theme.ansi[2], p.syntax_string),
                (theme.ansi[3], p.syntax_number),
                (theme.ansi[6], p.syntax_type),
            ] {
                let a = Lab::from_rgb(source);
                let b = Lab::from_rgb(resolved);
                let source_chroma = a.a.hypot(a.b);
                let resolved_chroma = b.a.hypot(b.b);
                // The tolerance accounts for the final RGB channel quantization.
                assert!(
                    resolved_chroma <= source_chroma + 0.004,
                    "{}: foreground became more vivid",
                    theme.name
                );
                if source_chroma > 0.02 && resolved_chroma > 0.02 {
                    let angle = (a.a * b.a + a.b * b.b) / (source_chroma * resolved_chroma);
                    assert!(
                        angle.clamp(-1., 1.).acos() < 0.08,
                        "{}: foreground hue drifted",
                        theme.name
                    );
                }
            }
        }
    }

    #[test]
    fn primary_secondary_and_metadata_text_keep_their_reading_hierarchy() {
        for theme in presets() {
            let p = Palette::for_theme(theme.id, false);
            let [text, muted, faint] = [p.text, p.muted, p.faint].map(luminance);
            if theme.light {
                assert!(
                    text <= muted && muted <= faint,
                    "{} inverted text priority",
                    theme.name
                );
            } else {
                assert!(
                    text >= muted && muted >= faint,
                    "{} inverted text priority",
                    theme.name
                );
            }
        }
    }

    #[test]
    fn standard_color_conversion_roundtrips_representative_palette_colors() {
        for color in [
            0x000000, 0xffffff, 0x808080, 0xff0000, 0x00ff00, 0x0000ff, 0xf4b8e4, 0x224fbc,
        ] {
            assert_eq!(
                Lab::from_rgb(color).to_rgb(),
                color,
                "#{color:06x} failed roundtrip"
            );
        }
    }
}
