#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Posting {
    pub doc_id: u32,
    pub term_frequency: u32,
    pub positions: Vec<u32>,
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

    fn encode_vbyte(mut val: u32, buf: &mut Vec<u8>) {
        while val >= 0x80 {
            buf.push(((val & 0x7F) as u8) | 0x80);
            val >>= 7;
        }
        buf.push((val & 0x7F) as u8);
    }

    #[test]
    fn test_decode_vbyte_values() {
        let test_values = [0u32, 1, 127, 128, 16383, 16384, 65535, 1_000_000, u32::MAX];
        let mut buf = Vec::new();
        for &val in &test_values {
            encode_vbyte(val, &mut buf);
        }

        let mut offset = 0;
        let mut decoded = Vec::new();
        while let Some(v) = decode_vbyte(&buf, &mut offset) {
            decoded.push(v);
        }

        assert_eq!(decoded, test_values);
        assert_eq!(offset, buf.len());
    }

    #[test]
    fn test_decode_vbyte_truncated_and_overflow() {
        let mut offset = 0;
        assert_eq!(decode_vbyte(&[], &mut offset), None);

        offset = 0;
        assert_eq!(decode_vbyte(&[0x80], &mut offset), None);

        offset = 0;
        let malformed = [0x80, 0x80, 0x80, 0x80, 0x80, 0x80];
        assert_eq!(decode_vbyte(&malformed, &mut offset), None);
    }

    #[test]
    fn test_postings_iterator_delta_decoding() {
        let mut buf = Vec::new();
        // doc 10, tf 2, pos: [4, 9] (pos_delta: 4, 5)
        encode_vbyte(10, &mut buf);
        encode_vbyte(2, &mut buf);
        encode_vbyte(4, &mut buf);
        encode_vbyte(5, &mut buf);

        // doc 15 (delta 5), tf 1, pos: [12] (pos_delta: 12)
        encode_vbyte(5, &mut buf);
        encode_vbyte(1, &mut buf);
        encode_vbyte(12, &mut buf);

        // doc 40 (delta 25), tf 3, pos: [0, 6, 20] (pos_delta: 0, 6, 14)
        encode_vbyte(25, &mut buf);
        encode_vbyte(3, &mut buf);
        encode_vbyte(0, &mut buf);
        encode_vbyte(6, &mut buf);
        encode_vbyte(14, &mut buf);

        let iter = PostingsIterator::new(&buf, 3);
        let postings: Vec<Posting> = iter.collect();

        assert_eq!(
            postings,
            vec![
                Posting {
                    doc_id: 10,
                    term_frequency: 2,
                    positions: vec![4, 9],
                },
                Posting {
                    doc_id: 15,
                    term_frequency: 1,
                    positions: vec![12],
                },
                Posting {
                    doc_id: 40,
                    term_frequency: 3,
                    positions: vec![0, 6, 20],
                },
            ]
        );
    }

    #[test]
    fn test_postings_iterator_first_doc_zero() {
        let mut buf = Vec::new();
        // doc 0, tf 1, pos: [0]
        encode_vbyte(0, &mut buf);
        encode_vbyte(1, &mut buf);
        encode_vbyte(0, &mut buf);

        // doc 3 (delta 3), tf 2, pos: [5, 10] (pos_delta: 5, 5)
        encode_vbyte(3, &mut buf);
        encode_vbyte(2, &mut buf);
        encode_vbyte(5, &mut buf);
        encode_vbyte(5, &mut buf);

        let iter = PostingsIterator::new(&buf, 2);
        let postings: Vec<Posting> = iter.collect();

        assert_eq!(
            postings,
            vec![
                Posting {
                    doc_id: 0,
                    term_frequency: 1,
                    positions: vec![0],
                },
                Posting {
                    doc_id: 3,
                    term_frequency: 2,
                    positions: vec![5, 10],
                },
            ]
        );
    }

    #[test]
    fn test_postings_iterator_empty() {
        let iter = PostingsIterator::new(&[], 0);
        let results: Vec<Posting> = iter.collect();
        assert!(results.is_empty());
    }
}
