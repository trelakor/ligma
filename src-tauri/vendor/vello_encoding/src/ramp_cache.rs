// Copyright 2022 the Vello Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

use std::collections::HashMap;
use std::hash::Hash;
use std::hash::Hasher;

use peniko::color::cache_key::{BitEq, BitHash, CacheKey};
use peniko::color::{ColorSpaceTag, HueDirection, Srgb};
use peniko::{ColorStop, ColorStops, InterpolationAlphaSpace};

const N_SAMPLES: usize = 512;
const RETAINED_COUNT: usize = 64;

/// Data and dimensions for a set of resolved gradient ramps.
#[derive(Copy, Clone, Debug, Default)]
pub struct Ramps<'a> {
    pub data: &'a [u32],
    pub width: u32,
    pub height: u32,
}

/// Cache key for gradient color ramps based on color-affecting properties.
#[derive(Debug, Clone)]
struct GradientCacheKey {
    /// The color stops (offsets + colors).
    pub stops: ColorStops,
    /// Interpolation alpha space.
    pub interpolation_alpha_space: InterpolationAlphaSpace,
    /// Color space the colors mix in, and which way hues go round in it.
    pub space: (ColorSpaceTag, HueDirection),
}

impl BitHash for GradientCacheKey {
    fn bit_hash<H: Hasher>(&self, state: &mut H) {
        self.stops.bit_hash(state);
        core::mem::discriminant(&self.interpolation_alpha_space).hash(state);
        self.space.0.hash(state);
        core::mem::discriminant(&self.space.1).hash(state);
    }
}

impl BitEq for GradientCacheKey {
    fn bit_eq(&self, other: &Self) -> bool {
        self.stops.bit_eq(&other.stops)
            && self.interpolation_alpha_space == other.interpolation_alpha_space
            && self.space == other.space
    }
}

#[derive(Default)]
pub(crate) struct RampCache {
    epoch: u64,
    map: HashMap<CacheKey<GradientCacheKey>, (u32, u64)>,
    data: Vec<u32>,
}

impl RampCache {
    pub(crate) fn maintain(&mut self) {
        self.epoch += 1;
        if self.map.len() > RETAINED_COUNT {
            self.map
                .retain(|_key, value| value.0 < RETAINED_COUNT as u32);
            self.data.truncate(RETAINED_COUNT * N_SAMPLES);
        }
    }

    pub(crate) fn add(
        &mut self,
        interpolation_alpha_space: InterpolationAlphaSpace,
        space: (ColorSpaceTag, HueDirection),
        stops: &[ColorStop],
    ) -> u32 {
        let key = CacheKey(GradientCacheKey {
            stops: stops.into(),
            interpolation_alpha_space,
            space,
        });
        if let Some(entry) = self.map.get_mut(&key) {
            entry.1 = self.epoch;
            entry.0
        } else if self.map.len() < RETAINED_COUNT {
            let id = (self.data.len() / N_SAMPLES) as u32;
            self.data
                .extend(make_ramp(stops, interpolation_alpha_space, space));
            self.map.insert(key, (id, self.epoch));
            id
        } else {
            let mut reuse = None;
            for (stops, (id, epoch)) in &self.map {
                if *epoch + 2 < self.epoch {
                    reuse = Some((stops.to_owned(), *id));
                    break;
                }
            }
            if let Some((old_stops, id)) = reuse {
                self.map.remove(&old_stops);
                let start = id as usize * N_SAMPLES;
                for (dst, src) in self.data[start..start + N_SAMPLES]
                    .iter_mut()
                    .zip(make_ramp(stops, interpolation_alpha_space, space))
                {
                    *dst = src;
                }
                self.map.insert(key, (id, self.epoch));
                id
            } else {
                let id = (self.data.len() / N_SAMPLES) as u32;
                self.data
                    .extend(make_ramp(stops, interpolation_alpha_space, space));
                self.map.insert(key, (id, self.epoch));
                id
            }
        }
    }

    pub(crate) fn ramps(&self) -> Ramps<'_> {
        Ramps {
            data: &self.data,
            width: N_SAMPLES as u32,
            height: (self.data.len() / N_SAMPLES) as u32,
        }
    }
}

fn make_ramp(
    stops: &[ColorStop],
    interpolation_alpha_space: InterpolationAlphaSpace,
    (cs, hue): (ColorSpaceTag, HueDirection),
) -> impl Iterator<Item = u32> + '_ {
    let mut last_u = 0.0;
    let mut last_c = stops[0].color;
    // Before the first stop is its color, wherever that stop is
    let mut this_u = stops[0].offset.max(0.0);
    let mut this_c = last_c;
    let mut j = 0;
    (0..N_SAMPLES).map(move |i| {
        let u = (i as f32) / (N_SAMPLES - 1) as f32;
        while u > this_u {
            last_u = this_u;
            last_c = this_c;
            if let Some(s) = stops.get(j + 1) {
                this_u = s.offset;
                this_c = s.color;
                j += 1;
            } else {
                break;
            }
        }
        let du = this_u - last_u;
        let c = if du < 1e-9 {
            this_c.to_alpha_color::<Srgb>()
        } else {
            let t = (u - last_u) / du;
            let (a, b) = (last_c.to_alpha_color::<Srgb>(), this_c.to_alpha_color::<Srgb>());
            match (interpolation_alpha_space, cs) {
                (InterpolationAlphaSpace::Premultiplied, ColorSpaceTag::Srgb) => a.lerp(b, t, HueDirection::default()),
                (InterpolationAlphaSpace::Unpremultiplied, ColorSpaceTag::Srgb) => a + (b - a) * t,
                (InterpolationAlphaSpace::Premultiplied, _) => last_c.interpolate(this_c, cs, hue).eval(t).to_alpha_color::<Srgb>(),
                (InterpolationAlphaSpace::Unpremultiplied, _) => {
                    last_c.interpolate_unpremultiplied(this_c, cs, hue).eval(t).to_alpha_color::<Srgb>()
                }
            }
        };
        c.premultiply().to_rgba8().to_u32()
    })
}
