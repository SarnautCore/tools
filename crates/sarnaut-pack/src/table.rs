//! The `.sptbl` container: a 40-byte header, a key index, a row index and the
//! concatenated protobuf rows. The byte layout is specified by ADR 0029.

use anyhow::{Result, bail, ensure};

pub const MAGIC: [u8; 4] = *b"SPK1";
pub const FORMAT_VERSION: u16 = 1;
pub const HEADER_BYTES: u32 = 40;
pub const KEY_ENTRY_BYTES: u32 = 12;
/// Bit 0 of `flags`: the key index is present.
pub const FLAG_KEY_INDEX: u16 = 1;

/// One encoded row plus the canonical id it is looked up by.
pub struct Row {
    pub key: String,
    pub bytes: Vec<u8>,
}

/// First 8 bytes of BLAKE3-256 over the canonical id, read little-endian.
pub fn key_hash(canonical_id: &str) -> u64 {
    let digest = blake3::hash(canonical_id.as_bytes());
    let mut head = [0u8; 8];
    head.copy_from_slice(&digest.as_bytes()[..8]);
    u64::from_le_bytes(head)
}

/// Serialises `rows` into one table. Rows must already be sorted by `key`,
/// bytewise ascending, with no duplicates: that ordering is what makes
/// `pack_id` a pure function of the authored content.
pub fn encode(row_type_id: i32, rows: &[Row]) -> Result<Vec<u8>> {
    for pair in rows.windows(2) {
        ensure!(
            pair[0].key < pair[1].key,
            "rows are not sorted by canonical id: {:?} then {:?}",
            pair[0].key,
            pair[1].key
        );
    }
    let row_count = u32::try_from(rows.len()).context_rows()?;

    let key_index_offset = HEADER_BYTES;
    let row_index_offset = key_index_offset + KEY_ENTRY_BYTES * row_count;
    let row_data_offset = row_index_offset + 4 * (row_count + 1);
    let row_data_bytes =
        u32::try_from(rows.iter().map(|row| row.bytes.len()).sum::<usize>()).context_rows()?;

    let mut output = Vec::with_capacity((row_data_offset + row_data_bytes) as usize);
    output.extend_from_slice(&MAGIC);
    output.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    output.extend_from_slice(&FLAG_KEY_INDEX.to_le_bytes());
    output.extend_from_slice(&row_type_id.to_le_bytes());
    output.extend_from_slice(&row_count.to_le_bytes());
    output.extend_from_slice(&key_index_offset.to_le_bytes());
    output.extend_from_slice(&row_index_offset.to_le_bytes());
    output.extend_from_slice(&row_data_offset.to_le_bytes());
    output.extend_from_slice(&row_data_bytes.to_le_bytes());
    output.extend_from_slice(&[0u8; 8]);

    let mut keys: Vec<(u64, u32)> = rows
        .iter()
        .enumerate()
        .map(|(ordinal, row)| (key_hash(&row.key), ordinal as u32))
        .collect();
    keys.sort_unstable();
    for (hash, ordinal) in keys {
        output.extend_from_slice(&hash.to_le_bytes());
        output.extend_from_slice(&ordinal.to_le_bytes());
    }

    let mut cursor = 0u32;
    output.extend_from_slice(&cursor.to_le_bytes());
    for row in rows {
        cursor += row.bytes.len() as u32;
        output.extend_from_slice(&cursor.to_le_bytes());
    }
    for row in rows {
        output.extend_from_slice(&row.bytes);
    }
    Ok(output)
}

/// What a structurally valid table says about itself.
#[derive(Debug)]
pub struct TableView {
    pub row_type_id: i32,
    pub row_count: u32,
}

/// Applies every structural rule ADR 0029 requires a reader to check, so that
/// `sarnaut-pack verify` rejects exactly what the shard would reject.
pub fn validate(bytes: &[u8]) -> Result<TableView> {
    ensure!(
        bytes.len() >= HEADER_BYTES as usize,
        "table is {} bytes, shorter than the {HEADER_BYTES}-byte header",
        bytes.len()
    );
    ensure!(
        bytes[..4] == MAGIC,
        "table does not start with the SPK1 magic"
    );

    let format_version = u16_at(bytes, 4);
    ensure!(
        format_version == FORMAT_VERSION,
        "table format_version is {format_version}, want {FORMAT_VERSION}"
    );
    let flags = u16_at(bytes, 6);
    ensure!(
        flags & !FLAG_KEY_INDEX == 0,
        "table sets reserved flag bits: {flags:#06x}"
    );
    ensure!(
        flags & FLAG_KEY_INDEX == FLAG_KEY_INDEX,
        "table has no key index"
    );
    ensure!(
        bytes[32..40] == [0u8; 8],
        "table reserved header bytes are not zero"
    );

    let row_type_id = u32_at(bytes, 8) as i32;
    let row_count = u32_at(bytes, 12);
    let key_index_offset = u32_at(bytes, 16);
    let row_index_offset = u32_at(bytes, 20);
    let row_data_offset = u32_at(bytes, 24);
    let row_data_bytes = u32_at(bytes, 28);

    let key_index_bytes = KEY_ENTRY_BYTES
        .checked_mul(row_count)
        .ok_or_else(|| anyhow::anyhow!("table row_count {row_count} overflows the key index"))?;
    let row_index_bytes = row_count
        .checked_add(1)
        .and_then(|entries| entries.checked_mul(4))
        .ok_or_else(|| anyhow::anyhow!("table row_count {row_count} overflows the row index"))?;

    let regions = [
        ("key index", key_index_offset, key_index_bytes),
        ("row index", row_index_offset, row_index_bytes),
        ("row data", row_data_offset, row_data_bytes),
    ];
    for (name, offset, length) in regions {
        ensure!(
            offset >= HEADER_BYTES,
            "table {name} at {offset} overlaps the header"
        );
        let end = offset
            .checked_add(length)
            .ok_or_else(|| anyhow::anyhow!("table {name} extent overflows"))?;
        ensure!(
            end as usize <= bytes.len(),
            "table {name} ends at {end}, past the {} byte file",
            bytes.len()
        );
    }
    for (left, right) in [(0, 1), (0, 2), (1, 2)] {
        let (first, second) = (regions[left], regions[right]);
        let overlaps = first.2 > 0
            && second.2 > 0
            && first.1 < second.1.saturating_add(second.2)
            && second.1 < first.1.saturating_add(first.2);
        ensure!(
            !overlaps,
            "table {} and {} regions overlap",
            first.0,
            second.0
        );
    }
    ensure!(
        (row_data_offset + row_data_bytes) as usize == bytes.len(),
        "table is {} bytes, want exactly {}",
        bytes.len(),
        row_data_offset + row_data_bytes
    );

    let mut previous_end = 0u32;
    for index in 0..=row_count {
        let value = u32_at(bytes, (row_index_offset + index * 4) as usize);
        if index == 0 {
            ensure!(value == 0, "table row index starts at {value}, want 0");
        } else {
            ensure!(
                value > previous_end,
                "table row index is not strictly increasing at entry {index}"
            );
        }
        previous_end = value;
    }
    ensure!(
        previous_end == row_data_bytes,
        "table row index ends at {previous_end}, want row_data_bytes {row_data_bytes}"
    );

    let mut previous = (0u64, 0u32);
    for index in 0..row_count {
        let base = (key_index_offset + index * KEY_ENTRY_BYTES) as usize;
        let entry = (u64_at(bytes, base), u32_at(bytes, base + 8));
        ensure!(
            entry.1 < row_count,
            "table key index entry {index} points at row {}, past row_count {row_count}",
            entry.1
        );
        if index > 0 {
            ensure!(
                entry >= previous,
                "table key index is not sorted at entry {index}"
            );
        }
        previous = entry;
    }

    if row_count == 0 && row_data_bytes != 0 {
        bail!("table has no rows but {row_data_bytes} bytes of row data");
    }
    Ok(TableView {
        row_type_id,
        row_count,
    })
}

/// Borrows each encoded row of a table that has already passed [`validate`].
pub fn rows(bytes: &[u8]) -> Result<Vec<&[u8]>> {
    let view = validate(bytes)?;
    let row_index_offset = u32_at(bytes, 20) as usize;
    let row_data_offset = u32_at(bytes, 24) as usize;
    Ok((0..view.row_count as usize)
        .map(|index| {
            let start = u32_at(bytes, row_index_offset + index * 4) as usize;
            let end = u32_at(bytes, row_index_offset + (index + 1) * 4) as usize;
            &bytes[row_data_offset + start..row_data_offset + end]
        })
        .collect())
}

fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

fn u64_at(bytes: &[u8], offset: usize) -> u64 {
    let mut value = [0u8; 8];
    value.copy_from_slice(&bytes[offset..offset + 8]);
    u64::from_le_bytes(value)
}

/// Turns the `u32` conversion failures in [`encode`] into one shared message.
trait TableSizeError<T> {
    fn context_rows(self) -> Result<T>;
}

impl<T> TableSizeError<T> for std::result::Result<T, std::num::TryFromIntError> {
    fn context_rows(self) -> Result<T> {
        self.map_err(|_| anyhow::anyhow!("table exceeds the 4 GiB the .sptbl offsets can address"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<Row> {
        vec![
            Row {
                key: "item.a".to_string(),
                bytes: vec![1, 2, 3],
            },
            Row {
                key: "item.b".to_string(),
                bytes: vec![4],
            },
            Row {
                key: "item.c".to_string(),
                bytes: vec![5, 6],
            },
        ]
    }

    #[test]
    fn an_encoded_table_validates_and_reads_back() {
        let bytes = encode(2, &sample()).expect("encode");
        let view = validate(&bytes).expect("validate");
        assert_eq!(view.row_type_id, 2);
        assert_eq!(view.row_count, 3);
        assert_eq!(
            rows(&bytes).expect("rows"),
            vec![&[1u8, 2, 3][..], &[4][..], &[5, 6][..]]
        );
    }

    #[test]
    fn the_header_is_forty_bytes_and_the_key_index_follows_it() {
        let bytes = encode(1, &sample()).expect("encode");
        assert_eq!(&bytes[..4], b"SPK1");
        assert_eq!(u16_at(&bytes, 4), FORMAT_VERSION);
        assert_eq!(u16_at(&bytes, 6), FLAG_KEY_INDEX);
        assert_eq!(u32_at(&bytes, 16), HEADER_BYTES);
        assert_eq!(u32_at(&bytes, 20), HEADER_BYTES + KEY_ENTRY_BYTES * 3);
    }

    #[test]
    fn the_key_index_is_sorted_by_hash() {
        let bytes = encode(1, &sample()).expect("encode");
        let base = u32_at(&bytes, 16) as usize;
        let hashes: Vec<u64> = (0..3)
            .map(|index| u64_at(&bytes, base + index * KEY_ENTRY_BYTES as usize))
            .collect();
        let mut sorted = hashes.clone();
        sorted.sort_unstable();
        assert_eq!(hashes, sorted);
    }

    #[test]
    fn unsorted_rows_are_refused() {
        let mut rows = sample();
        rows.swap(0, 2);
        let error = encode(1, &rows).expect_err("encode should refuse unsorted rows");
        assert!(format!("{error:#}").contains("not sorted"), "{error:#}");
    }

    #[test]
    fn a_wrong_magic_is_refused() {
        let mut bytes = encode(1, &sample()).expect("encode");
        bytes[0] = b'X';
        assert!(validate(&bytes).is_err());
    }

    #[test]
    fn a_reserved_flag_bit_is_refused() {
        let mut bytes = encode(1, &sample()).expect("encode");
        bytes[6] = 0b0000_0011;
        let error = validate(&bytes).expect_err("validate should refuse unknown flags");
        assert!(format!("{error:#}").contains("reserved flag"), "{error:#}");
    }

    #[test]
    fn a_non_zero_reserved_field_is_refused() {
        let mut bytes = encode(1, &sample()).expect("encode");
        bytes[39] = 1;
        assert!(validate(&bytes).is_err());
    }

    #[test]
    fn a_truncated_table_is_refused() {
        let bytes = encode(1, &sample()).expect("encode");
        let error = validate(&bytes[..bytes.len() - 1]).expect_err("validate should refuse");
        assert!(
            format!("{error:#}").contains("past the 97 byte file"),
            "{error:#}"
        );
    }

    #[test]
    fn an_empty_table_round_trips() {
        let bytes = encode(3, &[]).expect("encode");
        let view = validate(&bytes).expect("validate");
        assert_eq!(view.row_count, 0);
        assert!(rows(&bytes).expect("rows").is_empty());
    }
}
