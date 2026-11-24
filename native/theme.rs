use crate::Result;

pub type Color = [u8; 3];
pub const THEMES: &[(&str, &str)] = &[
    ("ocean", "#173d66"),
    ("teal", "#087e8b"),
    ("forest", "#256348"),
    ("plum", "#743c78"),
    ("terracotta", "#a34f35"),
    ("graphite", "#374151"),
];

#[derive(Clone, Debug)]
pub struct Theme {
    pub primary: Color,
    pub on_primary: Color,
    pub heading: Color,
    pub accent: Color,
    pub text: Color,
    pub muted: Color,
    pub surface: Color,
    pub border: Color,
}

fn mix(a: Color, b: Color, t: f32) -> Color {
    std::array::from_fn(|i| (a[i] as f32 * (1.0 - t) + b[i] as f32 * t).round() as u8)
}

fn luminance(c: Color) -> f32 {
    let values = c.map(|v| {
        let s = v as f32 / 255.0;
        if s <= 0.04045 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    });
    values[0] * 0.2126 + values[1] * 0.7152 + values[2] * 0.0722
}

fn contrast(a: Color, b: Color) -> f32 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

fn readable(mut color: Color) -> Color {
    while contrast(color, [255; 3]) < 4.5 {
        color = mix(color, [0; 3], 0.1);
    }
    color
}

impl Theme {
    pub fn new(name: &str, custom: Option<&str>) -> Result<Self> {
        let default = THEMES
            .iter()
            .find(|(n, _)| *n == name)
            .ok_or_else(|| format!("Unknown theme: {name}. Use --list-themes"))?
            .1;
        let color = custom
            .unwrap_or(default)
            .strip_prefix('#')
            .unwrap_or(custom.unwrap_or(default));
        if color.len() != 6 || !color.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Err("Invalid color: use #RRGGBB (example: '#087e8b')".into());
        }
        let primary = [
            u8::from_str_radix(&color[0..2], 16)?,
            u8::from_str_radix(&color[2..4], 16)?,
            u8::from_str_radix(&color[4..6], 16)?,
        ];
        let white = [255; 3];
        let black = [0; 3];
        let on_primary = if contrast(primary, white) >= contrast(primary, black) {
            white
        } else {
            black
        };
        let heading = readable(primary);
        Ok(Self {
            primary,
            on_primary,
            heading,
            accent: readable(mix(primary, [40, 160, 150], 0.18)),
            text: mix(primary, black, 0.7),
            muted: mix(heading, white, 0.25),
            surface: mix(primary, white, 0.95),
            border: mix(primary, white, 0.8),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn six_themes_and_extreme_custom_colors_are_readable() {
        assert!(THEMES.len() >= 6);
        for (name, _) in THEMES {
            for custom in [
                None,
                Some("#ffffff"),
                Some("#000000"),
                Some("#00ff00"),
                Some("#ffff00"),
            ] {
                let theme = Theme::new(name, custom).unwrap();
                assert!(contrast(theme.primary, theme.on_primary) >= 4.5);
                assert!(contrast(theme.heading, [255; 3]) >= 4.5);
                assert!(contrast(theme.text, theme.surface) >= 4.5);
            }
        }
        assert!(Theme::new("ocean", Some("xyzxyz")).is_err());
        assert!(Theme::new("inesistente", None).is_err());
    }
}
