//! CPU-side mip-chain construction for role-aware RGBA8 shader images.
//!
//! This module deliberately owns only byte-level filtering. It does not name a
//! GPU image, sampler, or render pipeline, so both the render-free terrain
//! baker and the render-side authored-image binder can use the same filtering
//! rules without creating a second implementation at either boundary.

/// The colour space represented by an RGBA8 image's texels.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Rgba8MipMode {
    /// Decode RGB from sRGB, average in linear space, then encode back to sRGB.
    /// Alpha is always averaged as a linear coverage/value channel.
    SrgbColor,
    /// Average all channels as already-linear values.
    Linear,
    /// Decode RGB as a [-1, 1] vector, average and renormalize it, then encode
    /// it back to [0, 1]. Alpha is averaged as a scalar channel.
    Normal,
}

/// Build a complete RGBA8 mip chain, with level zero first.
///
/// The input must contain exactly `width * height * 4` bytes. Dimensions do
/// not have to be powers of two: each next level uses the GPU texture rule
/// `max(1, size / 2)` and clamps its four source samples at the edge. Invalid
/// dimensions, lengths, or allocation sizes return `None` instead of panicking.
pub fn rgba8_mip_chain(
    base: Vec<u8>,
    width: usize,
    height: usize,
    mode: Rgba8MipMode,
) -> Option<(Vec<u8>, u32)> {
    if width == 0 || height == 0 {
        return None;
    }
    let base_len = width.checked_mul(height)?.checked_mul(4)?;
    if base.len() != base_len {
        return None;
    }

    let mut dimensions = Vec::new();
    let (mut level_width, mut level_height) = (width, height);
    let mut total_len = 0usize;
    loop {
        let level_len = level_width.checked_mul(level_height)?.checked_mul(4)?;
        total_len = total_len.checked_add(level_len)?;
        dimensions.push((level_width, level_height));
        if level_width == 1 && level_height == 1 {
            break;
        }
        // WebGPU uses floor-halving for logical mip extents. Ceil-halving
        // creates CPU levels that have no corresponding GPU subresource for
        // odd-sized textures and makes the descriptor invalid.
        level_width = (level_width / 2).max(1);
        level_height = (level_height / 2).max(1);
    }

    let mut all = Vec::with_capacity(total_len);
    all.extend_from_slice(&base);
    all.resize(total_len, 0);

    // RGBA8 has only 256 possible encoded channel values. Evaluate the same
    // transfer function once per byte value, preserving its native f32 result
    // while avoiding repeated nonlinear decoding for every source texel.
    let srgb_decoded: Option<[f32; 256]> = (mode == Rgba8MipMode::SrgbColor)
        .then(|| std::array::from_fn(|value| srgb_to_linear(value as f32 / 255.0)));

    let mut previous_offset = 0usize;
    for (&(previous_width, previous_height), &(next_width, next_height)) in
        dimensions.iter().zip(dimensions.iter().skip(1))
    {
        let next_offset = previous_offset.checked_add(
            previous_width
                .checked_mul(previous_height)?
                .checked_mul(4)?,
        )?;
        for y in 0..next_height {
            for x in 0..next_width {
                let sample_x = x * 2;
                let sample_y = y * 2;
                let x0 = sample_x.min(previous_width - 1);
                let x1 = (sample_x + 1).min(previous_width - 1);
                let y0 = sample_y.min(previous_height - 1);
                let y1 = (sample_y + 1).min(previous_height - 1);
                let samples = [
                    previous_offset + (y0 * previous_width + x0) * 4,
                    previous_offset + (y0 * previous_width + x1) * 4,
                    previous_offset + (y1 * previous_width + x0) * 4,
                    previous_offset + (y1 * previous_width + x1) * 4,
                ];
                let output = next_offset + (y * next_width + x) * 4;

                let mut rgb = [0.0f32; 3];
                match mode {
                    Rgba8MipMode::SrgbColor => {
                        let decoded = srgb_decoded
                            .as_ref()
                            .expect("sRGB mode initializes its decode table");
                        for channel in 0..3 {
                            rgb[channel] = samples
                                .iter()
                                .map(|&index| decoded[all[index + channel] as usize])
                                .sum::<f32>()
                                / 4.0;
                            rgb[channel] = linear_to_srgb(rgb[channel]);
                        }
                    }
                    Rgba8MipMode::Linear => {
                        for channel in 0..3 {
                            rgb[channel] = samples
                                .iter()
                                .map(|&index| all[index + channel] as f32 / 255.0)
                                .sum::<f32>()
                                / 4.0;
                        }
                    }
                    Rgba8MipMode::Normal => {
                        for &index in &samples {
                            for channel in 0..3 {
                                rgb[channel] += all[index + channel] as f32 / 255.0 * 2.0 - 1.0;
                            }
                        }
                        let length = (rgb[0] * rgb[0] + rgb[1] * rgb[1] + rgb[2] * rgb[2]).sqrt();
                        let normal = if length > f32::EPSILON {
                            [rgb[0] / length, rgb[1] / length, rgb[2] / length]
                        } else {
                            [0.0, 0.0, 1.0]
                        };
                        for channel in 0..3 {
                            rgb[channel] = normal[channel] * 0.5 + 0.5;
                        }
                    }
                }
                for channel in 0..3 {
                    all[output + channel] = (rgb[channel].clamp(0.0, 1.0) * 255.0).round() as u8;
                }

                all[output + 3] = samples
                    .iter()
                    .map(|&index| all[index + 3] as u16)
                    .sum::<u16>()
                    .div_ceil(4) as u8;
            }
        }
        previous_offset = next_offset;
    }

    Some((all, dimensions.len() as u32))
}

fn srgb_to_linear(value: f32) -> f32 {
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(value: f32) -> f32 {
    if value <= 0.0031308 {
        value * 12.92
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_chain_averages_a_two_by_two_level() {
        let base = vec![
            0, 10, 20, 255, 20, 30, 40, 255, 40, 50, 60, 255, 60, 70, 80, 255,
        ];
        let (chain, levels) = rgba8_mip_chain(base, 2, 2, Rgba8MipMode::Linear).unwrap();

        assert_eq!(levels, 2);
        assert_eq!(&chain[16..], &[30, 40, 50, 255]);
    }

    #[test]
    fn srgb_chain_averages_in_linear_space() {
        let base = vec![
            0, 0, 0, 255, 255, 255, 255, 255, 0, 0, 0, 255, 255, 255, 255, 255,
        ];
        let (chain, _) = rgba8_mip_chain(base, 2, 2, Rgba8MipMode::SrgbColor).unwrap();

        // Linear 50% encoded as sRGB is approximately 188, not 128.
        assert!((187..=189).contains(&chain[16]));
        assert_eq!(&chain[16..20], &[chain[16], chain[16], chain[16], 255]);
    }

    #[test]
    fn srgb_chain_preserves_transfer_for_every_byte_pair() {
        let width = 512;
        let base_len = width * width * 4;
        let mut base = vec![0; base_len];
        let mut expected = vec![0; 256 * 256 * 4];
        for a in 0..256 {
            for b in 0..256 {
                let pixels = [
                    [a as u8, b as u8, 255, a as u8],
                    [b as u8, a as u8, 0, b as u8],
                    [255, 0, a as u8, 0],
                    [0, 255, b as u8, 255],
                ];
                for (i, pixel) in pixels.iter().enumerate() {
                    let offset = ((a * 2 + i / 2) * width + b * 2 + i % 2) * 4;
                    base[offset..offset + 4].copy_from_slice(pixel);
                }
                let output = &mut expected[(a * 256 + b) * 4..(a * 256 + b + 1) * 4];
                for channel in 0..3 {
                    let linear = pixels
                        .iter()
                        .map(|pixel| srgb_to_linear(pixel[channel] as f32 / 255.0))
                        .sum::<f32>()
                        / 4.0;
                    output[channel] =
                        (linear_to_srgb(linear).clamp(0.0, 1.0) * 255.0).round() as u8;
                }
                output[3] = pixels
                    .iter()
                    .map(|pixel| pixel[3] as u16)
                    .sum::<u16>()
                    .div_ceil(4) as u8;
            }
        }
        let (chain, levels) = rgba8_mip_chain(base, width, width, Rgba8MipMode::SrgbColor).unwrap();
        assert_eq!(levels, 10);
        assert_eq!(
            &chain[base_len..base_len + expected.len()],
            expected.as_slice()
        );
    }

    #[test]
    fn normal_chain_is_renormalized() {
        let base = vec![
            128, 128, 255, 255, 255, 128, 255, 255, 128, 128, 255, 255, 255, 128, 128, 255,
        ];
        let (chain, _) = rgba8_mip_chain(base, 2, 2, Rgba8MipMode::Normal).unwrap();
        let normal = [
            chain[16] as f32 / 255.0 * 2.0 - 1.0,
            chain[17] as f32 / 255.0 * 2.0 - 1.0,
            chain[18] as f32 / 255.0 * 2.0 - 1.0,
        ];
        let length = (normal[0] * normal[0] + normal[1] * normal[1] + normal[2] * normal[2]).sqrt();
        assert!((length - 1.0).abs() < 0.01);
        assert!(normal[2] > 0.0);
    }

    #[test]
    fn invalid_base_is_rejected() {
        assert!(rgba8_mip_chain(vec![0; 3], 1, 1, Rgba8MipMode::Linear).is_none());
        assert!(rgba8_mip_chain(Vec::new(), 0, 1, Rgba8MipMode::Linear).is_none());
    }

    #[test]
    fn non_power_of_two_dimensions_use_gpu_legal_mip_counts() {
        for (width, height, expected_levels) in [(2_500, 2_500, 12), (2_047, 2_047, 11)] {
            let base = vec![0; width * height * 4];
            let (chain, levels) =
                rgba8_mip_chain(base, width, height, Rgba8MipMode::Linear).unwrap();
            assert_eq!(levels, expected_levels);
            assert!(chain.len() > width * height * 4);
        }
    }

    #[test]
    fn rectangular_mips_halve_each_axis_independently() {
        let (chain, levels) =
            rgba8_mip_chain(vec![0; 1 * 129 * 4], 1, 129, Rgba8MipMode::Linear).unwrap();

        // 1x129 -> 1x64 -> 1x32 -> ... -> 1x1.
        assert_eq!(levels, 8);
        let expected_texels = 129 + 64 + 32 + 16 + 8 + 4 + 2 + 1;
        assert_eq!(chain.len(), expected_texels * 4);
    }
}
