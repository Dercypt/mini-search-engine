use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct Posting {
    pub doc_id: u32,
    pub term_frequency: u32,
    pub positions: Vec<u32>,
}

#[inline]
pub fn encode_vbyte(mut val: u32, buf: &mut Vec<u8>) {
    while val >= 0x80 {
        buf.push(((val & 0x7F) as u8) | 0x80);
        val >>= 7;
    }
    buf.push((val & 0x7F) as u8);
}

#[inline]
pub fn decode_vbyte(bytes: &[u8], offset: &mut usize) -> Option<u32> {
    let mut result = 0u32;
    let mut shift = 0;
    while *offset < bytes.len() {
        if shift > 28 {
            return None;
        }
        let byte = bytes[*offset];
        *offset += 1;
        result |= ((byte & 0x7F) as u32) << shift;
        if (byte & 0x80) == 0 {
            return Some(result);
        }
        shift += 7;
    }
    None
}

pub fn encode_postings(postings: &[Posting], buf: &mut Vec<u8>) {
    let mut last_doc_id = 0u32;
    for (i, p) in postings.iter().enumerate() {
        let delta = if i == 0 {
            p.doc_id
        } else {
            p.doc_id.saturating_sub(last_doc_id)
        };
        last_doc_id = p.doc_id;
        encode_vbyte(delta, buf);
        let tf = if !p.positions.is_empty() {
            p.positions.len() as u32
        } else {
            p.term_frequency
        };
        encode_vbyte(tf, buf);
        let mut last_pos = 0u32;
        for (j, &pos) in p.positions.iter().enumerate() {
            let pos_delta = if j == 0 {
                pos
            } else {
                pos.saturating_sub(last_pos)
            };
            last_pos = pos;
            encode_vbyte(pos_delta, buf);
        }
    }
}

pub struct PostingsIterator<'a> {
    slice: &'a [u8],
    offset: usize,
    last_doc_id: u32,
    remaining: usize,
    index: usize,
}

impl<'a> PostingsIterator<'a> {
    pub fn new(slice: &'a [u8], doc_freq: usize) -> Self {
        Self {
            slice,
            offset: 0,
            last_doc_id: 0,
            remaining: doc_freq,
            index: 0,
        }
    }
}

impl<'a> Iterator for PostingsIterator<'a> {
    type Item = Posting;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        let delta = decode_vbyte(self.slice, &mut self.offset)?;
        let doc_id = if self.index == 0 {
            delta
        } else {
            self.last_doc_id + delta
        };
        self.last_doc_id = doc_id;
        self.index += 1;
        self.remaining -= 1;
        let term_frequency = decode_vbyte(self.slice, &mut self.offset).unwrap_or(0);
        let mut positions = Vec::with_capacity(term_frequency as usize);
        let mut last_pos = 0u32;
        for j in 0..term_frequency {
            let pos_delta = decode_vbyte(self.slice, &mut self.offset)?;
            let pos = if j == 0 {
                pos_delta
            } else {
                last_pos + pos_delta
            };
            last_pos = pos;
            positions.push(pos);
        }
        Some(Posting {
            doc_id,
            term_frequency,
            positions,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vbyte_roundtrip_boundaries() {
        let test_values = [
            0u32,
            1,
            63,
            127, // 1 byte boundary (0x7F)
            128, // 2 byte boundary (0x80)
            129,
            255,
            256,
            16383, // 2 byte max ((1 << 14) - 1)
            16384, // 3 byte boundary (1 << 14)
            65535,
            65536,
            2097151,   // 3 byte max ((1 << 21) - 1)
            2097152,   // 4 byte boundary (1 << 21)
            268435455, // 4 byte max ((1 << 28) - 1)
            268435456, // 5 byte boundary (1 << 28)
            u32::MAX - 1,
            u32::MAX,
        ];

        for &val in &test_values {
            let mut buf = Vec::new();
            encode_vbyte(val, &mut buf);

            // Verify encoded length expectations
            let expected_bytes = match val {
                0..=0x7F => 1,
                0x80..=0x3FFF => 2,
                0x4000..=0x1F_FFFF => 3,
                0x20_0000..=0xFFF_FFFF => 4,
                _ => 5,
            };
            assert_eq!(
                buf.len(),
                expected_bytes,
                "Unexpected byte count for value {}",
                val
            );

            let mut offset = 0;
            let decoded = decode_vbyte(&buf, &mut offset);
            assert_eq!(decoded, Some(val), "Failed roundtrip for value {}", val);
            assert_eq!(
                offset,
                buf.len(),
                "Did not consume full buffer for value {}",
                val
            );
        }
    }

    #[test]
    fn test_vbyte_roundtrip_sequential_stream() {
        let values = [
            0u32,
            1,
            42,
            127,
            128,
            500,
            16384,
            99999,
            1_000_000,
            u32::MAX,
        ];
        let mut buf = Vec::new();

        for &v in &values {
            encode_vbyte(v, &mut buf);
        }

        let mut offset = 0;
        let mut decoded = Vec::new();
        while let Some(val) = decode_vbyte(&buf, &mut offset) {
            decoded.push(val);
        }

        assert_eq!(decoded, values);
        assert_eq!(offset, buf.len());
    }

    #[test]
    fn test_vbyte_decode_truncated_and_invalid() {
        let mut offset = 0;
        // Empty slice
        assert_eq!(decode_vbyte(&[], &mut offset), None);

        // Continuation bit set on single byte without terminating byte
        offset = 0;
        assert_eq!(decode_vbyte(&[0x80], &mut offset), None);

        // Incomplete sequence
        offset = 0;
        assert_eq!(decode_vbyte(&[0x81, 0x82], &mut offset), None);

        // Overflow: more than 5 bytes with continuation bit set (shift > 35)
        offset = 0;
        let malformed = [0x80, 0x80, 0x80, 0x80, 0x80, 0x80];
        assert_eq!(decode_vbyte(&malformed, &mut offset), None);
    }

    #[test]
    fn test_delta_decoding_roundtrip() {
        let original_postings = vec![
            Posting {
                doc_id: 10,
                term_frequency: 1,
                positions: vec![3],
            },
            Posting {
                doc_id: 15,
                term_frequency: 3,
                positions: vec![1, 5, 12],
            },
            Posting {
                doc_id: 42,
                term_frequency: 2,
                positions: vec![0, 7],
            },
            Posting {
                doc_id: 100,
                term_frequency: 4,
                positions: vec![2, 4, 6, 8],
            },
            Posting {
                doc_id: 105,
                term_frequency: 1,
                positions: vec![100],
            },
        ];

        let mut buf = Vec::new();
        encode_postings(&original_postings, &mut buf);

        let iter = PostingsIterator::new(&buf, original_postings.len());
        let decoded_postings: Vec<Posting> = iter.collect();

        assert_eq!(decoded_postings, original_postings);
    }

    #[test]
    fn test_delta_decoding_first_doc_zero() {
        let original_postings = vec![
            Posting {
                doc_id: 0,
                term_frequency: 3,
                positions: vec![0, 1, 2],
            },
            Posting {
                doc_id: 1,
                term_frequency: 2,
                positions: vec![5, 10],
            },
            Posting {
                doc_id: 2,
                term_frequency: 1,
                positions: vec![20],
            },
        ];

        let mut buf = Vec::new();
        encode_postings(&original_postings, &mut buf);

        let iter = PostingsIterator::new(&buf, original_postings.len());
        let decoded_postings: Vec<Posting> = iter.collect();

        assert_eq!(decoded_postings, original_postings);
    }

    #[test]
    fn test_delta_decoding_large_gaps() {
        let original_postings = vec![
            Posting {
                doc_id: 5,
                term_frequency: 1,
                positions: vec![10],
            },
            Posting {
                doc_id: 1_000,
                term_frequency: 2,
                positions: vec![50, 500],
            },
            Posting {
                doc_id: 100_000,
                term_frequency: 2,
                positions: vec![1_000, 20_000],
            },
            Posting {
                doc_id: 5_000_000,
                term_frequency: 3,
                positions: vec![100, 200_000, 1_000_000],
            },
        ];

        let mut buf = Vec::new();
        encode_postings(&original_postings, &mut buf);

        let iter = PostingsIterator::new(&buf, original_postings.len());
        let decoded: Vec<Posting> = iter.collect();

        assert_eq!(decoded, original_postings);
    }

    #[test]
    fn test_delta_decoding_empty_and_single_posting() {
        // Empty postings
        let empty_iter = PostingsIterator::new(&[], 0);
        let empty_results: Vec<Posting> = empty_iter.collect();
        assert!(empty_results.is_empty());

        // Single posting
        let single = vec![Posting {
            doc_id: 777,
            term_frequency: 2,
            positions: vec![42, 99],
        }];
        let mut buf = Vec::new();
        encode_postings(&single, &mut buf);

        let single_iter = PostingsIterator::new(&buf, single.len());
        let single_results: Vec<Posting> = single_iter.collect();
        assert_eq!(single_results, single);
    }

    #[test]
    fn test_delta_decoding_direct_accumulator() {
        let mut buf = Vec::new();
        encode_vbyte(10, &mut buf); // doc_id delta: 10
        encode_vbyte(2, &mut buf); // tf: 2
        encode_vbyte(5, &mut buf); // pos_delta: 5 -> pos: 5
        encode_vbyte(3, &mut buf); // pos_delta: 3 -> pos: 8
        encode_vbyte(5, &mut buf); // doc_id delta: 5 -> doc_id: 15
        encode_vbyte(1, &mut buf); // tf: 1
        encode_vbyte(12, &mut buf); // pos_delta: 12 -> pos: 12
        encode_vbyte(20, &mut buf); // doc_id delta: 20 -> doc_id: 35
        encode_vbyte(3, &mut buf); // tf: 3
        encode_vbyte(1, &mut buf); // pos_delta: 1 -> pos: 1
        encode_vbyte(4, &mut buf); // pos_delta: 4 -> pos: 5
        encode_vbyte(10, &mut buf); // pos_delta: 10 -> pos: 15

        let iter = PostingsIterator::new(&buf, 3);
        let decoded: Vec<Posting> = iter.collect();

        assert_eq!(
            decoded,
            vec![
                Posting {
                    doc_id: 10,
                    term_frequency: 2,
                    positions: vec![5, 8],
                },
                Posting {
                    doc_id: 15,
                    term_frequency: 1,
                    positions: vec![12],
                },
                Posting {
                    doc_id: 35,
                    term_frequency: 3,
                    positions: vec![1, 5, 15],
                },
            ]
        );
    }
}
