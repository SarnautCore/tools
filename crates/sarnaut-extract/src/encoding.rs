//! Text decoding for the localization `.txt` payloads a `loc_ref` points at.
//!
//! The classic trees store display strings as UTF-16LE with a byte-order mark, but a
//! handful of hand-edited files in the wider 1.1 tree are plain bytes. Guessing wrong
//! is worse than failing, so the ladder below only ever falls back to Windows-1251
//! after UTF-8 has been ruled out, and the chosen rung is reported so a surprising
//! decode is visible rather than silent.

/// How a byte payload was decoded, recorded so a surprising decode is auditable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Encoding {
    Utf16Le,
    Utf16Be,
    Utf8Bom,
    Utf8,
    Windows1251,
}

impl Encoding {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Utf16Le => "utf-16le",
            Self::Utf16Be => "utf-16be",
            Self::Utf8Bom => "utf-8-bom",
            Self::Utf8 => "utf-8",
            Self::Windows1251 => "windows-1251",
        }
    }
}

/// Decode one localization payload, returning the text and the rung that decoded it.
pub(crate) fn decode(bytes: &[u8]) -> (String, Encoding) {
    match bytes {
        [0xFF, 0xFE, rest @ ..] => (decode_utf16(rest, u16::from_le_bytes), Encoding::Utf16Le),
        [0xFE, 0xFF, rest @ ..] => (decode_utf16(rest, u16::from_be_bytes), Encoding::Utf16Be),
        [0xEF, 0xBB, 0xBF, rest @ ..] => (
            String::from_utf8_lossy(rest).into_owned(),
            Encoding::Utf8Bom,
        ),
        _ => match std::str::from_utf8(bytes) {
            Ok(text) => (text.to_owned(), Encoding::Utf8),
            Err(_) => (decode_windows_1251(bytes), Encoding::Windows1251),
        },
    }
}

fn decode_utf16(bytes: &[u8], word: fn([u8; 2]) -> u16) -> String {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| word([pair[0], pair[1]]))
        .collect();
    String::from_utf16_lossy(&units)
}

fn decode_windows_1251(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&byte| match byte {
            0x00..=0x7F => byte as char,
            0xC0..=0xFF => char::from_u32(0x0410 + u32::from(byte - 0xC0)).expect("cyrillic"),
            _ => WINDOWS_1251_HIGH[usize::from(byte - 0x80)],
        })
        .collect()
}

/// Windows-1251 code points for `0x80..=0xBF`; `0xC0..=0xFF` is the contiguous
/// Cyrillic block and is computed instead of tabulated.
const WINDOWS_1251_HIGH: [char; 64] = [
    '\u{0402}', '\u{0403}', '\u{201A}', '\u{0453}', '\u{201E}', '\u{2026}', '\u{2020}', '\u{2021}',
    '\u{20AC}', '\u{2030}', '\u{0409}', '\u{2039}', '\u{040A}', '\u{040C}', '\u{040B}', '\u{040F}',
    '\u{0452}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}', '\u{2022}', '\u{2013}', '\u{2014}',
    '\u{FFFD}', '\u{2122}', '\u{0459}', '\u{203A}', '\u{045A}', '\u{045C}', '\u{045B}', '\u{045F}',
    '\u{00A0}', '\u{040E}', '\u{045E}', '\u{0408}', '\u{00A4}', '\u{0490}', '\u{00A6}', '\u{00A7}',
    '\u{0401}', '\u{00A9}', '\u{0404}', '\u{00AB}', '\u{00AC}', '\u{00AD}', '\u{00AE}', '\u{0407}',
    '\u{00B0}', '\u{00B1}', '\u{0406}', '\u{0456}', '\u{0491}', '\u{00B5}', '\u{00B6}', '\u{00B7}',
    '\u{0451}', '\u{2116}', '\u{0454}', '\u{00BB}', '\u{0458}', '\u{0405}', '\u{0455}', '\u{0457}',
];

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16le(text: &str) -> Vec<u8> {
        let mut bytes = vec![0xFF, 0xFE];
        for unit in text.encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        bytes
    }

    #[test]
    fn picks_the_encoding_from_the_byte_order_mark_and_falls_back_to_cyrillic() {
        assert_eq!(
            decode(&utf16le("Зомби")),
            ("Зомби".to_owned(), Encoding::Utf16Le)
        );

        let mut big_endian = vec![0xFE, 0xFF];
        for unit in "Зомби".encode_utf16() {
            big_endian.extend_from_slice(&unit.to_be_bytes());
        }
        assert_eq!(decode(&big_endian), ("Зомби".to_owned(), Encoding::Utf16Be));

        let mut utf8_bom = vec![0xEF, 0xBB, 0xBF];
        utf8_bom.extend_from_slice("Зомби".as_bytes());
        assert_eq!(decode(&utf8_bom), ("Зомби".to_owned(), Encoding::Utf8Bom));

        assert_eq!(decode(b"Rat"), ("Rat".to_owned(), Encoding::Utf8));

        // 0xC7 0xEE 0xEC 0xE1 0xE8 is "Зомби" in Windows-1251 and is not valid UTF-8.
        assert_eq!(
            decode(&[0xC7, 0xEE, 0xEC, 0xE1, 0xE8]),
            ("Зомби".to_owned(), Encoding::Windows1251)
        );
    }

    #[test]
    fn tolerates_a_trailing_odd_byte_in_a_utf16_payload() {
        let mut bytes = utf16le("Rat");
        bytes.push(0x00);
        assert_eq!(decode(&bytes).0, "Rat");
    }
}
