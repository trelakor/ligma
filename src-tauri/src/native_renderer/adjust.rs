//! Image adjustments: the seven sliders of an image paint (exposure,
//! contrast, saturation, temperature, tint, highlights and shadows, each -1
//! to 1), applied to the decoded image before it's drawn, as Figma's
//! renderer applies them

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

    /// Applies the adjustments to `rgba`, straight alpha, in place, as
    /// Figma's renderer does: on colors raised to the power 2.2, highlights
    /// and shadows as a shift of exposure, temperature, tint and contrast in
    /// Lab, then exposure and saturation. Contrast reaches the renderer at
    /// 0.3 of its slider, the rest as they are
    pub fn apply(&self, rgba: &mut [u8]) {
        let to_linear: Vec<f32> = (0..256).map(|i| (i as f32 / 255.0).powf(2.2)).collect();
        for pixel in rgba.chunks_exact_mut(4) {
            let c = [0, 1, 2].map(|i| to_linear[pixel[i] as usize]);
            let c = self.color(c);
            for i in 0..3 {
                pixel[i] = (c[i].clamp(0.0, 1.0).powf(1.0 / 2.2) * 255.0 + 0.5) as u8;
            }
        }
    }

    /// One color, after the power 2.2, adjusted
    fn color(&self, c: [f32; 3]) -> [f32; 3] {
        const THRESHOLD: f32 = 0.5;
        let contrast = self.contrast * 0.3;
        // Highlights and shadows, how far the color's light is from the middle
        let d = luma(c) - THRESHOLD.powf(2.2);
        let k = if d > 0.0 { self.highlights } else { -self.shadows * 0.999 };
        let shift = 0.75 * d / (1.0 + 4.0 * d * d).sqrt() * k;
        let c = self.lab(c, contrast, THRESHOLD);
        let c = exposure(c, self.exposure + shift);
        saturate(c, self.saturation.clamp(-1.0, 1.0))
    }

    /// Temperature and tint move a and b, contrast moves L about the
    /// threshold, softly when it's raised
    fn lab(&self, c: [f32; 3], contrast: f32, threshold: f32) -> [f32; 3] {
        const WHITE: [f32; 3] = [0.95047, 1.0, 1.08883];
        let xyz = [
            (0.4124564 * c[0] + 0.3575761 * c[1] + 0.1804375 * c[2]) / WHITE[0],
            0.2126729 * c[0] + 0.7151522 * c[1] + 0.0721750 * c[2],
            (0.0193339 * c[0] + 0.1191920 * c[1] + 0.9503041 * c[2]) / WHITE[2],
        ]
        .map(f32::cbrt);
        let l = 116.0 * xyz[1] - 16.0;
        let a = 500.0 * (xyz[0] - xyz[1]) + 30.0 * self.tint;
        let b = 200.0 * (xyz[1] - xyz[2]) + 30.0 * self.temperature;
        let t = threshold * 100.0;
        let l = if contrast < 0.0 {
            let flat = (l - t) * 0.2 + 50.0;
            flat + (l - flat) * (contrast + 1.0)
        } else {
            let x = ((l - (t - 30.0)) / 60.0).clamp(0.0, 1.0);
            let steep = x * x * x * (x * (x * 6.0 - 15.0) + 10.0) * 100.0;
            l + (steep - l) * contrast
        };
        let fy = (l + 16.0) / 116.0;
        let [x, y, z] = [a / 500.0 + fy, fy, fy - b / 200.0].map(|v| v * v * v);
        let (x, z) = (x * WHITE[0], z * WHITE[2]);
        [
            3.2404542 * x - 1.5371385 * y - 0.4985314 * z,
            -0.9692660 * x + 1.8760108 * y + 0.0415560 * z,
            0.0556434 * x - 0.2040259 * y + 1.0572252 * z,
        ]
        .map(|v| v.clamp(0.0, 1.0))
    }
}

fn luma(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

/// Brightens or darkens along a curve that keeps white and black, with
/// colors pulled toward their light first and pushed back out after, more so
/// when darkening
fn exposure(c: [f32; 3], e: f32) -> [f32; 3] {
    let y = 10f32.powf(2.0 * e) - 1.0;
    let mut light = luma(c);
    let push = 3.0 * (1.0 + light * light * e.min(0.0) / (0.2 - e.min(0.0)));
    let mut c = c.map(|v| v + (light - v) * 0.75);
    if e < 0.0 {
        c = c.map(|v| v * (1.0 + e / 16.0));
        light *= 1.0 + e / 16.0;
    }
    let c = c.map(|v| v * (y + 1.0) / (v * y + 1.0));
    let light = light * (y + 1.0) / (light * y + 1.0);
    c.map(|v| (v + (v - light) * push).clamp(0.0, 1.0))
}

/// Less saturated toward gray, more toward the most it can be before a
/// channel clips, at most twice as far from gray
fn saturate(c: [f32; 3], s: f32) -> [f32; 3] {
    let light = luma(c);
    if s <= 0.0 {
        return c.map(|v| v - (light - v) * s);
    }
    let min = c[0].min(c[1]).min(c[2]);
    let max = c[0].max(c[1]).max(c[2]);
    let room = (light / (light - min + 1e-6)).min((1.0 - light) / (max - light + 1e-6));
    let k = (1.0 + (room - 1.0) * s).min(2.0);
    c.map(|v| light + (v - light) * k)
}
