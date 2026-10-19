//! Color and spacing tokens. Every color in the UI comes from here.

use eframe::egui::{self, Color32, CornerRadius, Stroke, Theme, Visuals};

#[derive(Clone, Copy)]
pub struct Tokens {
    pub bg: Color32,
    pub surface: Color32,
    pub surface_alt: Color32,
    pub border: Color32,
    pub text: Color32,
    pub text_2: Color32,
    pub muted: Color32,
    pub accent: Color32,
    pub on_accent: Color32,
    pub good: Color32,
    pub warning: Color32,
    pub critical: Color32,
    /// Sequential ramp end points for magnitude (low recedes into the surface).
    pub seq_low: Color32,
    pub seq_high: Color32,
}

const fn hex(v: u32) -> Color32 {
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

pub const LIGHT: Tokens = Tokens {
    bg: hex(0xf3f3f1),
    surface: hex(0xfcfcfb),
    surface_alt: hex(0xefeeeb),
    border: hex(0xe0dfdb),
    text: hex(0x0b0b0b),
    text_2: hex(0x52514e),
    muted: hex(0x7c7b76),
    accent: hex(0x2a78d6),
    on_accent: hex(0xffffff),
    good: hex(0x008300),
    warning: hex(0xb57a00),
    critical: hex(0xd23c3b),
    seq_low: hex(0xe3eefb),
    seq_high: hex(0x104281),
};

pub const DARK: Tokens = Tokens {
    bg: hex(0x121211),
    surface: hex(0x1a1a19),
    surface_alt: hex(0x242422),
    border: hex(0x2f2f2c),
    text: hex(0xf0efec),
    text_2: hex(0xc3c2b7),
    muted: hex(0x908f88),
    accent: hex(0x3987e5),
    on_accent: hex(0xffffff),
    good: hex(0x2fa82f),
    warning: hex(0xe0a020),
    critical: hex(0xe66767),
    seq_low: hex(0x1f2a38),
    seq_high: hex(0x86b6ef),
};

pub fn tokens(ctx: &egui::Context) -> Tokens {
    match ctx.theme() {
        Theme::Dark => DARK,
        Theme::Light => LIGHT,
    }
}

pub const RADIUS: u8 = 6;
pub const GAP: f32 = 12.0;

/// Linear blend along the sequential ramp, `t` in 0..=1.
pub fn seq(t: &Tokens, v: f64) -> Color32 {
    let v = v.clamp(0.0, 1.0) as f32;
    let lerp = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * v).round() as u8;
    Color32::from_rgb(
        lerp(t.seq_low.r(), t.seq_high.r()),
        lerp(t.seq_low.g(), t.seq_high.g()),
        lerp(t.seq_low.b(), t.seq_high.b()),
    )
}

fn visuals(t: &Tokens, dark: bool) -> Visuals {
    let mut v = if dark { Visuals::dark() } else { Visuals::light() };
    v.panel_fill = t.bg;
    v.window_fill = t.surface;
    v.extreme_bg_color = t.surface;
    v.faint_bg_color = t.surface_alt;
    v.window_stroke = Stroke::new(1.0, t.border);
    v.override_text_color = Some(t.text);
    v.hyperlink_color = t.accent;
    v.selection.bg_fill = t.accent.gamma_multiply(0.25);
    v.selection.stroke = Stroke::new(1.0, t.accent);
    let r = CornerRadius::same(RADIUS);
    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.corner_radius = r;
    }
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, t.border);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, t.text_2);
    v.widgets.inactive.bg_fill = t.surface_alt;
    v.widgets.inactive.weak_bg_fill = t.surface_alt;
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, t.text);
    v.widgets.hovered.bg_fill = t.border;
    v.widgets.hovered.weak_bg_fill = t.border;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, t.muted);
    v.widgets.active.bg_fill = t.accent;
    v.widgets.active.weak_bg_fill = t.accent;
    v.widgets.active.fg_stroke = Stroke::new(1.0, t.on_accent);
    v
}

pub fn install(ctx: &egui::Context) {
    ctx.set_visuals_of(Theme::Light, visuals(&LIGHT, false));
    ctx.set_visuals_of(Theme::Dark, visuals(&DARK, true));
    ctx.all_styles_mut(|s| {
        s.spacing.item_spacing = egui::vec2(8.0, 6.0);
        s.spacing.button_padding = egui::vec2(10.0, 5.0);
        s.spacing.interact_size.y = 26.0;
        use egui::{FontId, TextStyle};
        s.text_styles.insert(TextStyle::Heading, FontId::proportional(20.0));
        s.text_styles.insert(TextStyle::Body, FontId::proportional(14.0));
        s.text_styles.insert(TextStyle::Button, FontId::proportional(14.0));
        s.text_styles.insert(TextStyle::Small, FontId::proportional(12.0));
    });
}

/// A plain card: surface fill, hairline border, no shadow.
pub fn card(t: &Tokens) -> egui::Frame {
    egui::Frame::new()
        .fill(t.surface)
        .stroke(Stroke::new(1.0, t.border))
        .corner_radius(CornerRadius::same(RADIUS + 2))
        .inner_margin(egui::Margin::same(14))
}
