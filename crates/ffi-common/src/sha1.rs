//! SHA-1 digest for libfaust cache keys.
//!
//! # Source provenance (C++)
//! - `compiler/generator/sha_key.hh`, `generateSHA1(const std::string&)`
//!
//! # Why this must be a real SHA-1
//! The digest is a *cache identity*, not a checksum. `expandCDSPFromString`
//! and friends return it to hosts that use it to key compiled factories, and
//! `faustwasm` compares keys computed by different processes — potentially one
//! C++ and one Rust. A stand-in that happens to be 64 hex characters satisfies
//! the buffer contract while quietly answering "different program" for
//! identical sources, or worse, "same program" for different ones.
//!
//! # Digest format
//! C++ formats the digest with the alphabet `"0123456789ABCDEF"`
//! (`sha_key.hh:302`), so libfaust keys are **uppercase** hex — unlike almost
//! every other SHA-1 rendering. [`sha1_hex`] matches it, because a key that
//! differs only in case is still a different string to a host comparing two
//! keys. The 40 characters are written into the 64-character buffer the C API
//! documents; the remaining room is not filled, exactly as in C++.
//!
//! # Not a security primitive
//! SHA-1 is broken for collision resistance. It is used here only because the
//! reference implementation defines the key format, and a cache key is not an
//! authentication token. Nothing in this crate should use it to attest to
//! anything.

use std::fmt::Write as _;

/// SHA-1 initial state (FIPS 180-4 §5.3.1).
const INITIAL_STATE: [u32; 5] = [
    0x6745_2301,
    0xEFCD_AB89,
    0x98BA_DCFE,
    0x1032_5476,
    0xC3D2_E1F0,
];

/// Round constants, one per 20-round group.
const ROUND_CONSTANTS: [u32; 4] = [0x5A82_7999, 0x6ED9_EBA1, 0x8F1B_BCDC, 0xCA62_C1D6];

/// Returns the uppercase hex SHA-1 digest of `data`.
///
/// Matches C++ `generateSHA1` character for character — including its
/// uppercase alphabet — so keys computed on either side of the FFI boundary
/// compare equal for identical input.
#[must_use]
pub fn sha1_hex(data: &[u8]) -> String {
    let digest = sha1(data);
    let mut out = String::with_capacity(40);
    for byte in digest {
        let _ = write!(out, "{byte:02X}");
    }
    out
}

/// Returns the raw 20-byte SHA-1 digest of `data`.
#[must_use]
pub fn sha1(data: &[u8]) -> [u8; 20] {
    let mut state = INITIAL_STATE;

    // The padded message is the input, a `0x80` byte, zeros, and the original
    // bit length as a big-endian u64 — assembled here rather than copied into
    // one buffer so hashing a large source does not double its memory.
    let bit_length = (data.len() as u64).wrapping_mul(8);
    let mut tail = Vec::with_capacity(128);
    tail.push(0x80u8);
    let remainder = (data.len() + 1) % 64;
    let padding = if remainder <= 56 {
        56 - remainder
    } else {
        120 - remainder
    };
    tail.extend(std::iter::repeat_n(0u8, padding));
    tail.extend_from_slice(&bit_length.to_be_bytes());

    let mut block = [0u8; 64];
    let mut filled = 0usize;
    for byte in data.iter().chain(tail.iter()) {
        block[filled] = *byte;
        filled += 1;
        if filled == 64 {
            compress(&mut state, &block);
            filled = 0;
        }
    }
    debug_assert_eq!(filled, 0, "padding must complete the final block");

    let mut digest = [0u8; 20];
    for (index, word) in state.iter().enumerate() {
        digest[index * 4..index * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    digest
}

/// Applies one 64-byte block to the running state (FIPS 180-4 §6.1.2).
fn compress(state: &mut [u32; 5], block: &[u8; 64]) {
    let mut schedule = [0u32; 80];
    for (index, chunk) in block.chunks_exact(4).enumerate() {
        schedule[index] = u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
    }
    for index in 16..80 {
        let mixed =
            schedule[index - 3] ^ schedule[index - 8] ^ schedule[index - 14] ^ schedule[index - 16];
        schedule[index] = mixed.rotate_left(1);
    }

    let [mut a, mut b, mut c, mut d, mut e] = *state;
    for (index, word) in schedule.iter().enumerate() {
        let (mixed, constant) = match index / 20 {
            0 => ((b & c) | (!b & d), ROUND_CONSTANTS[0]),
            1 => (b ^ c ^ d, ROUND_CONSTANTS[1]),
            2 => ((b & c) | (b & d) | (c & d), ROUND_CONSTANTS[2]),
            _ => (b ^ c ^ d, ROUND_CONSTANTS[3]),
        };
        let temp = a
            .rotate_left(5)
            .wrapping_add(mixed)
            .wrapping_add(e)
            .wrapping_add(constant)
            .wrapping_add(*word);
        e = d;
        d = c;
        c = b.rotate_left(30);
        b = a;
        a = temp;
    }

    state[0] = state[0].wrapping_add(a);
    state[1] = state[1].wrapping_add(b);
    state[2] = state[2].wrapping_add(c);
    state[3] = state[3].wrapping_add(d);
    state[4] = state[4].wrapping_add(e);
}

#[cfg(test)]
mod tests {
    use super::sha1_hex;

    /// The published vectors are lowercase; libfaust keys are uppercase.
    /// Uppercasing here keeps the vector text recognizable against the RFC.
    fn expect(data: &[u8], canonical_lowercase: &str) {
        assert_eq!(
            sha1_hex(data),
            canonical_lowercase.to_ascii_uppercase(),
            "digest of {} bytes",
            data.len()
        );
    }

    #[test]
    fn matches_the_rfc_3174_vectors() {
        expect(b"abc", "a9993e364706816aba3e25717850c26c9cd0d89d");
        expect(
            b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq",
            "84983e441c3bd26ebaae4aa1f95129e5e54670f1",
        );
        expect(
            &b"a".repeat(1_000_000),
            "34aa973cd4c4daa4f61eeb2bdbad27316534016f",
        );
        expect(
            &b"0123456701234567012345670123456701234567012345670123456701234567".repeat(10),
            "dea356a2cddd90c7a7ecedc5ebb563934f460452",
        );
    }

    #[test]
    fn matches_the_empty_input_digest() {
        expect(b"", "da39a3ee5e6b4b0d3255bfef95601890afd80709");
    }

    #[test]
    fn the_alphabet_is_uppercase_like_libfaust() {
        // Not cosmetic: a host comparing a Rust key with a C++ key compares
        // strings, and `generateSHA1` formats with "0123456789ABCDEF".
        let key = sha1_hex(b"process = 0;");
        assert_eq!(key, "C066762BED1174200E3B0FD2A78499F438C5A5CC");
        assert!(
            key.chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        );
    }

    #[test]
    fn block_boundaries_are_padded_correctly() {
        // The three padding branches: a length whose padding fits in the
        // current block, one that forces a second block, and one already
        // block-aligned. Reference digests computed with `hashlib`.
        for (length, expected) in [
            (55, "c1c8bbdc22796e28c0e15163d20899b65621d65a"),
            (56, "c2db330f6083854c99d4b5bfb6e8f29f201be699"),
            (63, "03f09f5b158a7a8cdad920bddc29b81c18a551f5"),
            (64, "0098ba824b5c16427bd7a1122a5a442a25ec644d"),
            (119, "ee971065aaa017e0632a8ca6c77bb3bf8b1dfc56"),
            (120, "f34c1488385346a55709ba056ddd08280dd4c6d6"),
        ] {
            expect(&b"a".repeat(length), expected);
        }
    }
}
