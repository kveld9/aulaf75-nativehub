//! Host-rendered RGB lighting effects engine for AULA F75.
//!
//! Provides mathematical models for dynamic and static lighting effects
//! (Solid, Rainbow, Wave, Breathe, Stars, Off) calculated on the host and
//! streamed as Report 6 direct RGB frames.

/// 8-bit RGB color triplet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RGB(pub u8, pub u8, pub u8);

impl RGB {
    /// Create a new RGB color.
    pub fn new(r: u8, g: u8, b: u8) -> Self {
        Self(r, g, b)
    }

    /// Return RGB as a `(r, g, b)` tuple.
    pub fn to_tuple(&self) -> (u8, u8, u8) {
        (self.0, self.1, self.2)
    }

    /// Scale color channels by brightness multiplier clamped to `0.0..=1.0`.
    pub fn scale(&self, brightness: f32) -> RGB {
        let b = brightness.clamp(0.0, 1.0);
        RGB(
            ((self.0 as f32 * b).round() as u8).min(255),
            ((self.1 as f32 * b).round() as u8).min(255),
            ((self.2 as f32 * b).round() as u8).min(255),
        )
    }

    /// Convert HSV (Hue 0..360, Saturation 0..1, Value 0..1) to RGB.
    ///
    /// Guarantees:
    /// - h=0, s=1, v=1 -> (255, 0, 0)
    /// - h=120, s=1, v=1 -> (0, 255, 0)
    /// - h=240, s=1, v=1 -> (0, 0, 255)
    pub fn hsv_to_rgb(h: f64, s: f64, v: f64) -> RGB {
        let h_norm = ((h % 360.0) + 360.0) % 360.0;
        let s = s.clamp(0.0, 1.0);
        let v = v.clamp(0.0, 1.0);

        let c = v * s;
        let x = c * (1.0 - ((h_norm / 60.0) % 2.0 - 1.0).abs());
        let m = v - c;

        let (r1, g1, b1) = match (h_norm / 60.0).floor() as i32 {
            0 => (c, x, 0.0),
            1 => (x, c, 0.0),
            2 => (0.0, c, x),
            3 => (0.0, x, c),
            4 => (x, 0.0, c),
            _ => (c, 0.0, x),
        };

        let r = (((r1 + m) * 255.0).round() as u8).min(255);
        let g = (((g1 + m) * 255.0).round() as u8).min(255);
        let b = (((b1 + m) * 255.0).round() as u8).min(255);

        RGB(r, g, b)
    }
}

/// Host-rendered lighting effect definitions.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// Static single color across all mapped keys.
    Solid(RGB),
    /// Smooth cyclic spectrum shift across the whole board.
    Rainbow { period_s: f64 },
    /// Horizontal chromatic wave propagating across keys.
    Wave { period_s: f64 },
    /// Pulsing illumination fading between black and base color.
    Breathe { color: RGB, period_s: f64 },
    /// Deterministic twinkling starry night across keys.
    Stars { density: f32 },
    /// All key LEDs powered off (empty payload).
    Off,
}

impl Effect {
    /// Construct an effect from a case-insensitive name string and optional base color.
    pub fn from_name(name: &str, color: Option<RGB>) -> Result<Effect, String> {
        let trimmed = name.trim().to_ascii_lowercase();
        match trimmed.as_str() {
            "solid" => {
                let c = color.ok_or_else(|| "Effect 'solid' requires --color RRGGBB".to_string())?;
                Ok(Effect::Solid(c))
            }
            "rainbow" => Ok(Effect::Rainbow { period_s: 5.0 }),
            "wave" => Ok(Effect::Wave { period_s: 3.0 }),
            "breathe" => {
                let c = color.ok_or_else(|| "Effect 'breathe' requires --color RRGGBB".to_string())?;
                Ok(Effect::Breathe {
                    color: c,
                    period_s: 4.0,
                })
            }
            "stars" => Ok(Effect::Stars { density: 0.15 }),
            "off" => Ok(Effect::Off),
            _ => Err(format!(
                "Unknown effect name: '{}'. Available effects: solid, rainbow, wave, breathe, stars, off",
                name
            )),
        }
    }
}

/// Render an effect at timestamp `t_secs` for given key slots into `(slot, (r, g, b))` pairs.
pub fn render(effect: &Effect, t_secs: f64, key_slots: &[u8]) -> Vec<(u8, (u8, u8, u8))> {
    match effect {
        Effect::Solid(rgb) => key_slots.iter().map(|&slot| (slot, rgb.to_tuple())).collect(),
        Effect::Rainbow { period_s } => {
            let period = if *period_s <= 0.0 { 5.0 } else { *period_s };
            let hue = ((t_secs / period).fract() * 360.0 + 360.0) % 360.0;
            let rgb = RGB::hsv_to_rgb(hue, 1.0, 1.0).to_tuple();
            key_slots.iter().map(|&slot| (slot, rgb)).collect()
        }
        Effect::Wave { period_s } => {
            let period = if *period_s <= 0.0 { 3.0 } else { *period_s };
            key_slots
                .iter()
                .map(|&slot| {
                    let hue = (((t_secs / period) * 360.0 + (slot as f64 * 8.0)) % 360.0 + 360.0) % 360.0;
                    (slot, RGB::hsv_to_rgb(hue, 1.0, 1.0).to_tuple())
                })
                .collect()
        }
        Effect::Breathe { color, period_s } => {
            let period = if *period_s <= 0.0 { 4.0 } else { *period_s };
            let factor = (0.5 + 0.5 * (2.0 * std::f64::consts::PI * t_secs / period).sin()) as f32;
            let rgb = color.scale(factor).to_tuple();
            key_slots.iter().map(|&slot| (slot, rgb)).collect()
        }
        Effect::Stars { density } => {
            let dens = density.clamp(0.0, 1.0);
            key_slots
                .iter()
                .map(|&slot| {
                    let hash = (slot as u64).wrapping_mul(2654435761) ^ ((t_secs * 4.0) as u64);
                    let normalized = ((hash.wrapping_mul(11400714819323198485)) >> 32) as f32 / (u32::MAX as f32);
                    let rgb = if dens > 0.0 && normalized < dens {
                        let scale_val = (1.0 - (normalized / dens).min(1.0)).max(0.0);
                        RGB(255, 255, 255).scale(scale_val).to_tuple()
                    } else {
                        (0, 0, 0)
                    };
                    (slot, rgb)
                })
                .collect()
        }
        Effect::Off => Vec::new(),
    }
}

/// Apply global brightness scaling (clamped to 0.0..=1.0) to key slot color pairs.
pub fn apply_brightness(pairs: &[(u8, (u8, u8, u8))], b: f32) -> Vec<(u8, (u8, u8, u8))> {
    let factor = b.clamp(0.0, 1.0);
    pairs
        .iter()
        .map(|&(slot, (r, g, b_ch))| {
            let r_scaled = ((r as f32 * factor).round() as u8).min(255);
            let g_scaled = ((g as f32 * factor).round() as u8).min(255);
            let b_scaled = ((b_ch as f32 * factor).round() as u8).min(255);
            (slot, (r_scaled, g_scaled, b_scaled))
        })
        .collect()
}
