use egui::Color32;
use egui_shadcn::{ColorPalette, RadiusScale, ShadcnBaseColor, Theme};

/// macOSのシステムアクセントカラー(System Blue)。ライト/ダークで明度を分けている。
const APPLE_ACCENT_LIGHT: Color32 = Color32::from_rgb(0, 122, 255);
const APPLE_ACCENT_DARK: Color32 = Color32::from_rgb(10, 132, 255);

/// shadcnの既定値より一段大きい角丸スケール。macOSのコントロール/シートに近づけている。
fn apple_radius_scale() -> RadiusScale {
    RadiusScale {
        r1: 6.0,
        r2: 8.0,
        r3: 10.0,
        r4: 12.0,
        r5: 16.0,
        r6: 20.0,
    }
}

/// ライト/ダークモードに応じたApple風テーマを構築する。
/// ベースはshadcnのNeutralパレットを使い、アクセントカラーのみSystem Blueに上書きする。
pub fn build_theme(dark_mode: bool) -> Theme {
    let mut palette = if dark_mode {
        ColorPalette::shadcn_dark(ShadcnBaseColor::Neutral)
    } else {
        ColorPalette::shadcn_light(ShadcnBaseColor::Neutral)
    };

    let accent = if dark_mode {
        APPLE_ACCENT_DARK
    } else {
        APPLE_ACCENT_LIGHT
    };
    palette.primary = accent;
    palette.primary_foreground = Color32::WHITE;
    palette.ring = accent;
    palette.sidebar_primary = accent;
    palette.sidebar_ring = accent;

    Theme {
        palette,
        radius: apple_radius_scale(),
        ..Theme::default()
    }
}
