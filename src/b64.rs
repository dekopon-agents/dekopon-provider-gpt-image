//! The only base64 this guest performs: a validity scan, a length computation, and twelve bytes.
//!
//! A decoder is deliberately absent. The response carries an image as base64 inside JSON, up to
//! ~11 MiB of it, and the guest runs in a 64 MiB store. Decoding it would double the blob for no
//! purpose: the bytes are re-encoded by the gateway's attachment path anyway, the only property
//! worth checking is the PNG signature, and that lives in the first eight bytes — twelve
//! characters' worth of the first sixteen.

/// The eight-byte PNG signature every PNG file starts with.
pub(crate) const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];

/// Characters decoded for the signature check: four groups of four, twelve bytes out.
const PREFIX_CHARACTERS: usize = 16;

/// Returns the 6-bit value of one standard base64 alphabet character.
const fn sextet(character: u8) -> Option<u8> {
    Some(match character {
        b'A'..=b'Z' => character - b'A',
        b'a'..=b'z' => character - b'a' + 26,
        b'0'..=b'9' => character - b'0' + 52,
        b'+' => 62,
        b'/' => 63,
        _ => return None,
    })
}

/// Whether `value` is standard base64: alphabet characters, then at most two `=` of padding.
///
/// Scanning the whole string rather than sampling it buys two things the rest of the guest relies
/// on. A string in this alphabet needs no JSON escaping, so the serialized envelope is exactly the
/// skeleton plus the string's own length and the ceiling arithmetic is exact rather than a guess.
/// And the gateway, which does decode, cannot be handed something that fails there instead of here.
pub(crate) fn is_standard(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.is_empty() {
        return false;
    }
    let padding = bytes.iter().rev().take_while(|byte| **byte == b'=').count();
    if padding > 2 {
        return false;
    }
    // Padding exists only to complete a four-character group, so a padded string is a whole number
    // of groups long and an unpadded one may end in a partial group.
    if padding > 0 && !bytes.len().is_multiple_of(4) {
        return false;
    }
    bytes[..bytes.len() - padding]
        .iter()
        .all(|byte| sextet(*byte).is_some())
}

/// Returns the number of bytes `value` decodes to, without decoding it.
///
/// `None` when the length cannot be a base64 encoding of anything: a final group of one character
/// carries six bits, which no byte boundary can use.
pub(crate) fn decoded_len(value: &str) -> Option<usize> {
    let length = value.len();
    let padding = value
        .as_bytes()
        .iter()
        .rev()
        .take_while(|byte| **byte == b'=')
        .count();
    if padding > 2 {
        return None;
    }
    let groups = length / 4;
    match (length % 4, padding) {
        (0, _) => (groups * 3).checked_sub(padding),
        (2, 0) => Some(groups * 3 + 1),
        (3, 0) => Some(groups * 3 + 2),
        _ => None,
    }
}

/// Decodes the first twelve bytes and reports whether they open with the PNG signature.
///
/// Sixteen characters decode to exactly twelve bytes with no partial group and no padding to
/// consider, so this reads a fixed, tiny prefix of a blob that may be eleven megabytes long.
pub(crate) fn starts_with_png_signature(value: &str) -> bool {
    let Some(prefix) = value.as_bytes().get(..PREFIX_CHARACTERS) else {
        return false;
    };
    let mut decoded = [0_u8; 12];
    for (group, characters) in prefix.chunks_exact(4).enumerate() {
        let mut packed = 0_u32;
        for character in characters {
            let Some(sextet) = sextet(*character) else {
                return false;
            };
            packed = (packed << 6) | u32::from(sextet);
        }
        let bytes = packed.to_be_bytes();
        decoded[group * 3] = bytes[1];
        decoded[group * 3 + 1] = bytes[2];
        decoded[group * 3 + 2] = bytes[3];
    }
    decoded[..8] == PNG_SIGNATURE
}

#[cfg(test)]
pub(crate) mod tests {
    use super::{decoded_len, is_standard, starts_with_png_signature};

    /// A standard-alphabet encoding of `bytes`, written out so no decoder is needed to test one.
    pub(crate) fn encode(bytes: &[u8]) -> String {
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
        for chunk in bytes.chunks(3) {
            let mut packed = 0_u32;
            for (index, byte) in chunk.iter().enumerate() {
                packed |= u32::from(*byte) << (16 - index * 8);
            }
            for index in 0..4 {
                if index <= chunk.len() {
                    let sextet = (packed >> (18 - index * 6)) & 0b11_1111;
                    encoded.push(char::from(ALPHABET[sextet as usize]));
                } else {
                    encoded.push('=');
                }
            }
        }
        encoded
    }

    /// A base64 payload of exactly `characters` characters whose first twelve decoded bytes open
    /// with the PNG signature. Built by concatenation rather than by encoding megabytes, so a test
    /// that needs an eleven-megabyte blob does not spend its time in the encoder.
    pub(crate) fn png_payload(characters: usize) -> String {
        assert!(characters >= 16 && characters.is_multiple_of(4));
        let mut head = super::PNG_SIGNATURE.to_vec();
        head.extend_from_slice(&[0, 0, 0, 13]);
        let mut payload = encode(&head);
        assert_eq!(payload.len(), 16);
        payload.push_str(&"A".repeat(characters - 16));
        payload
    }

    #[test]
    fn accepts_the_standard_alphabet_and_rejects_everything_else() {
        assert!(is_standard("aGVsbG8="));
        assert!(is_standard("aGVsbG8"));
        assert!(is_standard("+/+/"));
        assert!(!is_standard(""));
        assert!(!is_standard("aGVs bG8="));
        assert!(!is_standard("aGVsbG8==="));
        assert!(!is_standard("aGVs\nbG8="));
        assert!(!is_standard("aGVsbG8-"));
        assert!(!is_standard("\"quoted\""));
        // One padding character completing a four-character group is ordinary base64; padding that
        // leaves the string short of a whole group is not.
        assert!(is_standard("aGV="));
        assert!(!is_standard("aG="));
        assert!(!is_standard("aGVsbG8sIHdvcmxkIQ="));
    }

    #[test]
    fn computes_decoded_lengths_without_decoding() {
        for length in 0_usize..=64 {
            let bytes = vec![0x41_u8; length];
            let encoded = encode(&bytes);
            assert_eq!(
                decoded_len(&encoded),
                Some(length),
                "{length} bytes encoded as {encoded}"
            );
        }
        assert_eq!(decoded_len("aGVsbG8"), Some(5));
        assert_eq!(decoded_len("a"), None);
        assert_eq!(decoded_len("aGVsbG8===="), None);
    }

    #[test]
    fn detects_the_png_signature_in_the_first_sixteen_characters() {
        let mut png = super::PNG_SIGNATURE.to_vec();
        png.extend_from_slice(&[0, 0, 0, 13, b'I', b'H', b'D', b'R']);
        assert!(starts_with_png_signature(&encode(&png)));

        let mut jpeg = vec![0xff_u8, 0xd8, 0xff, 0xe0];
        jpeg.extend_from_slice(&[0; 12]);
        assert!(!starts_with_png_signature(&encode(&jpeg)));

        // Too short to hold twelve bytes, and a prefix outside the alphabet.
        assert!(!starts_with_png_signature("iVBORw0K"));
        assert!(!starts_with_png_signature("iVBORw0KGgo!!!!!"));
    }
}
