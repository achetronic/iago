// SPDX-FileCopyrightText: 2026 Alby Hernández <hola@achetronic.com>
// SPDX-License-Identifier: Apache-2.0

//! Minimal WAV encoder.
//!
//! Parakeet accepts 16-bit PCM WAV, so we hand it exactly that: a 44-byte
//! canonical header followed by little-endian `i16` samples. Writing it here
//! keeps the dependency list free of audio-format crates.

/// Bytes in a canonical PCM WAV header.
const HEADER_BYTES: usize = 44;

/// Bits per sample we emit.
const BITS_PER_SAMPLE: u16 = 16;

const FORMAT_PCM: u16 = 1;

/// Encodes mono `f32` samples as a 16-bit PCM WAV file.
pub fn encode(samples: &[f32], sample_rate: u32, channels: u16) -> Vec<u8> {
    let channels = channels.max(1);
    let data_bytes = samples.len() * std::mem::size_of::<i16>();
    let byte_rate = sample_rate * u32::from(channels) * u32::from(BITS_PER_SAMPLE) / 8;
    let block_align = channels * BITS_PER_SAMPLE / 8;

    let mut out = Vec::with_capacity(HEADER_BYTES + data_bytes);

    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&((36 + data_bytes) as u32).to_le_bytes());
    out.extend_from_slice(b"WAVE");

    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&FORMAT_PCM.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&BITS_PER_SAMPLE.to_le_bytes());

    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data_bytes as u32).to_le_bytes());

    for sample in samples {
        let clamped = sample.clamp(-1.0, 1.0);
        let scaled = (clamped * f32::from(i16::MAX)) as i16;
        out.extend_from_slice(&scaled.to_le_bytes());
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_header_describes_the_payload() {
        let wav = encode(&[0.0, 0.5, -0.5], 16_000, 1);

        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(wav.len(), HEADER_BYTES + 6);

        let riff_size = u32::from_le_bytes([wav[4], wav[5], wav[6], wav[7]]);
        assert_eq!(riff_size as usize, wav.len() - 8);

        let data_size = u32::from_le_bytes([wav[40], wav[41], wav[42], wav[43]]);
        assert_eq!(data_size as usize, 6);
    }

    #[test]
    fn samples_are_clamped_before_quantisation() {
        let wav = encode(&[2.0, -2.0], 16_000, 1);
        let first = i16::from_le_bytes([wav[44], wav[45]]);
        let second = i16::from_le_bytes([wav[46], wav[47]]);

        assert_eq!(first, i16::MAX);
        assert_eq!(second, -i16::MAX);
    }
}
