use crate::storage::vbyte::decode_vbyte;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScoredDoc {
    pub doc_id: u32,
    pub score: f64,
}

impl Eq for ScoredDoc {}

impl PartialOrd for ScoredDoc {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ScoredDoc {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Min-heap ordering: reverse score so that smallest score has highest priority (popped first)
        other
            .score
            .partial_cmp(&self.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| self.doc_id.cmp(&other.doc_id))
    }
}

pub struct WandPostingCursor<'a> {
    slice: &'a [u8],
    offset: usize,
    last_doc_id: u32,
    remaining: usize,
    index: usize,
    pub current_doc_id: u32,
    pub current_tf: u32,
    pub idf: f64,
    pub max_score: f64,
    pub has_more: bool,
}

impl<'a> WandPostingCursor<'a> {
    pub fn new(slice: &'a [u8], doc_freq: usize, idf: f64, max_score: f64) -> Self {
        let mut cursor = Self {
            slice,
            offset: 0,
            last_doc_id: 0,
            remaining: doc_freq,
            index: 0,
            current_doc_id: 0,
            current_tf: 0,
            idf,
            max_score,
            has_more: false,
        };
        cursor.read_next();
        cursor
    }

    #[inline]
    pub fn read_next(&mut self) -> bool {
        if self.remaining == 0 {
            self.has_more = false;
            return false;
        }
        let Some(delta) = decode_vbyte(self.slice, &mut self.offset) else {
            self.has_more = false;
            return false;
        };
        let doc_id = if self.index == 0 {
            delta
        } else {
            self.last_doc_id + delta
        };
        self.last_doc_id = doc_id;
        self.index += 1;
        self.remaining -= 1;
        self.current_doc_id = doc_id;

        let tf = decode_vbyte(self.slice, &mut self.offset).unwrap_or(0);
        self.current_tf = tf;

        // Skip position deltas for this posting: tf VByte numbers
        for _ in 0..tf {
            while self.offset < self.slice.len() {
                let byte = self.slice[self.offset];
                self.offset += 1;
                if (byte & 0x80) == 0 {
                    break;
                }
            }
        }

        self.has_more = true;
        true
    }

    #[inline]
    pub fn advance_to(&mut self, target_doc_id: u32) -> bool {
        while self.has_more && self.current_doc_id < target_doc_id {
            self.read_next();
        }
        self.has_more
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::vbyte::encode_vbyte;

    #[test]
    fn test_wand_posting_cursor_advance_and_read() {
        let mut buf = Vec::new();
        encode_vbyte(5, &mut buf);
        encode_vbyte(1, &mut buf);
        encode_vbyte(0, &mut buf);

        encode_vbyte(7, &mut buf);
        encode_vbyte(2, &mut buf);
        encode_vbyte(3, &mut buf);
        encode_vbyte(5, &mut buf);

        encode_vbyte(18, &mut buf);
        encode_vbyte(1, &mut buf);
        encode_vbyte(10, &mut buf);

        encode_vbyte(15, &mut buf);
        encode_vbyte(1, &mut buf);
        encode_vbyte(2, &mut buf);

        let mut cursor = WandPostingCursor::new(&buf, 4, 2.0, 5.0);
        assert!(cursor.has_more);
        assert_eq!(cursor.current_doc_id, 5);
        assert_eq!(cursor.current_tf, 1);

        assert!(cursor.advance_to(20));
        assert_eq!(cursor.current_doc_id, 30);
        assert_eq!(cursor.current_tf, 1);

        assert!(cursor.advance_to(30));
        assert_eq!(cursor.current_doc_id, 30);

        assert!(cursor.advance_to(40));
        assert_eq!(cursor.current_doc_id, 45);

        assert!(!cursor.advance_to(50));
        assert!(!cursor.has_more);
    }

    #[test]
    fn test_wand_cursor_tracking_distinct_matching_docs() {
        use std::collections::HashSet;

        let mut buf1 = Vec::new();
        encode_vbyte(1, &mut buf1);
        encode_vbyte(1, &mut buf1);
        encode_vbyte(0, &mut buf1);

        encode_vbyte(4, &mut buf1);
        encode_vbyte(1, &mut buf1);
        encode_vbyte(0, &mut buf1);

        encode_vbyte(5, &mut buf1);
        encode_vbyte(1, &mut buf1);
        encode_vbyte(0, &mut buf1);

        let mut buf2 = Vec::new();
        encode_vbyte(5, &mut buf2);
        encode_vbyte(1, &mut buf2);
        encode_vbyte(0, &mut buf2);

        encode_vbyte(15, &mut buf2);
        encode_vbyte(1, &mut buf2);
        encode_vbyte(0, &mut buf2);

        let c1 = WandPostingCursor::new(&buf1, 3, 1.0, 1.0);
        let c2 = WandPostingCursor::new(&buf2, 2, 1.0, 1.0);
        let mut cursors = vec![c1, c2];

        let mut matching_docs = HashSet::new();

        while !cursors.is_empty() {
            cursors.sort_unstable_by_key(|c| c.current_doc_id);
            let pivot_doc = cursors[0].current_doc_id;

            if cursors.iter().all(|c| c.current_doc_id == pivot_doc) {
                matching_docs.insert(pivot_doc);
                for c in cursors.iter_mut() {
                    c.read_next();
                }
            } else {
                let doc_id = cursors[0].current_doc_id;
                matching_docs.insert(doc_id);
                cursors[0].read_next();
            }
            cursors.retain(|c| c.has_more);
        }

        assert_eq!(matching_docs.len(), 4);
        assert!(matching_docs.contains(&1));
        assert!(matching_docs.contains(&5));
        assert!(matching_docs.contains(&10));
        assert!(matching_docs.contains(&20));
    }
}
