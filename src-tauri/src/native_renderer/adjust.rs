//! Image adjustments: the seven sliders of an image paint (exposure,
//! contrast, saturation, temperature, tint, highlights and shadows, each -1
//! to 1), applied to the decoded image before it's drawn. Figma doesn't say
//! how it applies them, so these are fitted to its exports with the parity
//! harness's `compare`

use std::collections::HashMap;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Adjust {
    pub exposure: f32,
    pub contrast: f32,
    pub saturation: f32,
    pub temperature: f32,
    pub tint: f32,
    pub highlights: f32,
    pub shadows: f32,
}

impl Adjust {
    pub fn from_filters(filters: &HashMap<String, f64>) -> Self {
        let get = |name: &str| filters.get(name).copied().unwrap_or(0.0) as f32;
        Self {
            exposure: get("exposure"),
            contrast: get("contrast"),
            saturation: get("saturation"),
            temperature: get("temperature"),
            tint: get("tint"),
            highlights: get("highlights"),
            shadows: get("shadows"),
        }
    }

    fn values(&self) -> [f32; 7] {
        [self.exposure, self.contrast, self.saturation, self.temperature, self.tint, self.highlights, self.shadows]
    }

    /// For an image's cache key: the image's hash, then this
    pub fn key(&self) -> String {
        self.values().map(|value| format!("{value:.3}")).join(",")
    }

    pub fn parse(key: &str) -> Option<Self> {
        let values: Vec<f32> = key.split(',').map(|part| part.parse().ok()).collect::<Option<_>>()?;
        let [exposure, contrast, saturation, temperature, tint, highlights, shadows] = values.try_into().ok()?;
        Some(Self { exposure, contrast, saturation, temperature, tint, highlights, shadows })
    }

    /// Applies the adjustments to `rgba`, straight alpha, in place
    pub fn apply(&self, rgba: &mut [u8]) {
        let to_linear: Vec<f32> = (0..256).map(|i| srgb_to_linear(i as f32 / 255.0)).collect();
        let gain = 2f32.powf(self.exposure);
        let contrast = 1.0 + self.contrast;
        let saturation = 1.0 + self.saturation;
        // Warmer: more red, less blue. Tint: more magenta, less green
        let warm = [1.0 + 0.1 * self.temperature, 1.0, 1.0 - 0.1 * self.temperature];
        let tint = [1.0 + 0.05 * self.tint, 1.0 - 0.1 * self.tint, 1.0 + 0.05 * self.tint];
        for pixel in rgba.chunks_exact_mut(4) {
            let mut c = [0, 1, 2].map(|i| to_linear[pixel[i] as usize] * gain * warm[i] * tint[i]);
            let mut s = c.map(linear_to_srgb);
            let luma = 0.2126 * s[0] + 0.7152 * s[1] + 0.0722 * s[2];
            // Shadows lift or deepen the dark end, highlights the light end
            let shift = self.shadows * 0.25 * (1.0 - luma).powi(2) + self.highlights * 0.25 * luma.powi(2);
            for v in &mut s {
                *v = (*v - 0.5) * contrast + 0.5 + shift;
            }
            let luma = 0.2126 * s[0] + 0.7152 * s[1] + 0.0722 * s[2];
            for v in &mut s {
                *v = luma + (*v - luma) * saturation;
            }
            c = s;
            for i in 0..3 {
                pixel[i] = (c[i].clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            }
        }
    }
}

fn srgb_to_linear(v: f32) -> f32 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(v: f32) -> f32 {
    let v = v.max(0.0);
    if v <= 0.0031308 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}
