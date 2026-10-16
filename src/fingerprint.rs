//! The FNV-1a fold every compatibility tag is made with: the net protocol's fingerprint and the pen
//! cam's shot vocabulary. Byte-driven and integer-only, so every platform folds the same manifest to
//! the same tag. Not a security primitive.

/// The FNV-1a 64-bit offset basis, the seed a fold starts from.
pub(crate) const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;

/// FNV-1a over `bytes`, from `seed`.
pub(crate) const fn fnv1a_64(seed: u64, bytes: &[u8]) -> u64 {
    let mut hash = seed;
    let mut i = 0;
    while i < bytes.len() {
        hash ^= bytes[i] as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        i += 1;
    }
    hash
}

/// Fold one labeled manifest field, with delimiters around its value.
pub(crate) const fn fingerprint_field(hash: u64, label: &[u8], value: &[u8]) -> u64 {
    let after_label = fnv1a_64(hash, label);
    let after_label_separator = fnv1a_64(after_label, b"\0");
    let after_value = fnv1a_64(after_label_separator, value);
    fnv1a_64(after_value, b"\0")
}
