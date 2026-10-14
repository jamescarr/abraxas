//! Deterministic, portable randomness.
//!
//! Every random decision in the engine draws from a *substream* derived from the run seed, the
//! level depth, a purpose label and an attempt counter. The same inputs always give the same
//! stream, independent of the order in which other streams are consumed.

use rand::SeedableRng;
use rand::rngs::Xoshiro256PlusPlus;

/// The generator used everywhere in the engine.
pub type Rng = Xoshiro256PlusPlus;

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

fn splitmix64(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn fnv1a64(s: &str) -> u64 {
    s.bytes().fold(FNV_OFFSET, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(FNV_PRIME)
    })
}

/// A 64-bit value derived from `(seed, depth, purpose, attempt)`; also used as an AI request seed.
pub fn substream_u64(seed: u64, depth: u32, purpose: &str, attempt: u32) -> u64 {
    splitmix64(seed ^ splitmix64(u64::from(depth)) ^ fnv1a64(purpose) ^ (u64::from(attempt) << 32))
}

/// A generator seeded from `(seed, depth, purpose, attempt)`.
pub fn substream(seed: u64, depth: u32, purpose: &str, attempt: u32) -> Rng {
    Rng::seed_from_u64(substream_u64(seed, depth, purpose, attempt))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::RngExt;

    fn draw(rng: &mut Rng) -> Vec<u32> {
        (0..8).map(|_| rng.random_range(0..1000)).collect()
    }

    #[test]
    fn same_inputs_same_stream() {
        let a = draw(&mut substream(42, 3, "place", 0));
        let b = draw(&mut substream(42, 3, "place", 0));
        assert_eq!(a, b);
    }

    #[test]
    fn each_input_separates_the_stream() {
        let base = draw(&mut substream(42, 3, "place", 0));
        assert_ne!(base, draw(&mut substream(43, 3, "place", 0)));
        assert_ne!(base, draw(&mut substream(42, 4, "place", 0)));
        assert_ne!(base, draw(&mut substream(42, 3, "passage", 0)));
        assert_ne!(base, draw(&mut substream(42, 3, "place", 1)));
    }

    #[test]
    fn stream_values_are_pinned() {
        // Guards against accidental changes to the derivation, which would break saved runs.
        assert_eq!(
            substream_u64(0, 0, "", 0),
            splitmix64(splitmix64(0) ^ FNV_OFFSET)
        );
        assert_eq!(fnv1a64(""), FNV_OFFSET);
        assert_eq!(fnv1a64("a"), 0xaf63_dc4c_8601_ec8c);
    }
}
