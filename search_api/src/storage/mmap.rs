use fst::automaton::{Levenshtein, Str};
use fst::{Automaton, IntoStreamer, Map, MapBuilder, Streamer};
use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

pub struct TermEntry {
    pub postings_offset: u64,
    pub postings_len: u32,
    pub doc_freq: u32,
}

pub struct DocMetaRef<'a> {
    pub hex_id: &'a str,
    pub url: &'a str,
    pub title: &'a str,
    pub pagerank: f64,
}

pub struct MmapIndex {
    mmap: memmap2::Mmap,
    pub total_docs: u32,
    pub avg_doc_length: f64,
    pub num_terms: u32,
    doc_lengths_offset: usize,
    pagerank_offset: usize,
    doc_meta_index_offset: usize,
    doc_meta_data_offset: usize,
    terms_table_offset: usize,
    terms_strings_offset: usize,
    postings_offset: usize,
}

impl MmapIndex {
    pub fn open<P: AsRef<Path>>(path: P) -> std::io::Result<Self> {
        let file = File::open(path)?;
        let mmap = unsafe { memmap2::Mmap::map(&file)? };
        if mmap.len() < 128 || &mmap[0..8] != b"MSEIDX01" {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Invalid index magic header",
            ));
        }
        let total_docs = u32::from_le_bytes(mmap[8..12].try_into().unwrap());
        let avg_doc_length = f64::from_le_bytes(mmap[12..20].try_into().unwrap());
        let doc_lengths_offset = u64::from_le_bytes(mmap[20..28].try_into().unwrap()) as usize;
        let doc_meta_index_offset = u64::from_le_bytes(mmap[28..36].try_into().unwrap()) as usize;
        let doc_meta_data_offset = u64::from_le_bytes(mmap[36..44].try_into().unwrap()) as usize;
        let terms_table_offset = u64::from_le_bytes(mmap[44..52].try_into().unwrap()) as usize;
        let num_terms = u32::from_le_bytes(mmap[52..56].try_into().unwrap());
        let terms_strings_offset = u64::from_le_bytes(mmap[56..64].try_into().unwrap()) as usize;
        let postings_offset = u64::from_le_bytes(mmap[64..72].try_into().unwrap()) as usize;
        let pagerank_offset = if mmap.len() >= 88 {
            u64::from_le_bytes(mmap[80..88].try_into().unwrap()) as usize
        } else {
            0
        };

        Ok(Self {
            mmap,
            total_docs,
            avg_doc_length,
            num_terms,
            doc_lengths_offset,
            pagerank_offset,
            doc_meta_index_offset,
            doc_meta_data_offset,
            terms_table_offset,
            terms_strings_offset,
            postings_offset,
        })
    }

    #[inline]
    pub fn get_doc_length(&self, doc_id: u32) -> u32 {
        if doc_id >= self.total_docs {
            return 0;
        }
        let off = self.doc_lengths_offset + (doc_id as usize) * 4;
        u32::from_le_bytes(self.mmap[off..off + 4].try_into().unwrap())
    }

    #[inline]
    pub fn get_pagerank(&self, doc_id: u32) -> f64 {
        if doc_id >= self.total_docs || self.pagerank_offset == 0 {
            return if self.total_docs > 0 {
                1.0 / (self.total_docs as f64)
            } else {
                0.0
            };
        }
        let off = self.pagerank_offset + (doc_id as usize) * 8;
        if off + 8 > self.mmap.len() {
            return if self.total_docs > 0 {
                1.0 / (self.total_docs as f64)
            } else {
                0.0
            };
        }
        f64::from_le_bytes(self.mmap[off..off + 8].try_into().unwrap())
    }

    pub fn get_doc_meta(&self, doc_id: u32) -> Option<DocMetaRef<'_>> {
        if doc_id >= self.total_docs {
            return None;
        }
        let idx_off = self.doc_meta_index_offset + (doc_id as usize) * 8;
        let meta_rel_start =
            u64::from_le_bytes(self.mmap[idx_off..idx_off + 8].try_into().unwrap()) as usize;
        let meta_rel_end =
            u64::from_le_bytes(self.mmap[idx_off + 8..idx_off + 16].try_into().unwrap()) as usize;

        let start = self.doc_meta_data_offset + meta_rel_start;
        let end = self.doc_meta_data_offset + meta_rel_end;
        if end > self.mmap.len() || start >= end {
            return None;
        }
        let slice = &self.mmap[start..end];
        let mut pos = 0;
        if slice.len() < 2 {
            return None;
        }
        let hex_id_len = u16::from_le_bytes([slice[pos], slice[pos + 1]]) as usize;
        pos += 2;
        let hex_id = std::str::from_utf8(&slice[pos..pos + hex_id_len]).ok()?;
        pos += hex_id_len;

        if pos + 2 > slice.len() {
            return None;
        }
        let url_len = u16::from_le_bytes([slice[pos], slice[pos + 1]]) as usize;
        pos += 2;
        let url = std::str::from_utf8(&slice[pos..pos + url_len]).ok()?;
        pos += url_len;

        if pos + 2 > slice.len() {
            return None;
        }
        let title_len = u16::from_le_bytes([slice[pos], slice[pos + 1]]) as usize;
        pos += 2;
        let title = std::str::from_utf8(&slice[pos..pos + title_len]).ok()?;

        let pagerank = self.get_pagerank(doc_id);

        Some(DocMetaRef {
            hex_id,
            url,
            title,
            pagerank,
        })
    }

    #[inline]
    pub fn get_term_entry(&self, term_idx: usize) -> Option<TermEntry> {
        if term_idx >= self.num_terms as usize {
            return None;
        }
        let off = self.terms_table_offset + term_idx * 16;
        if off + 16 > self.mmap.len() {
            return None;
        }
        let postings_offset = u64::from_le_bytes(self.mmap[off..off + 8].try_into().unwrap());
        let postings_len = u32::from_le_bytes(self.mmap[off + 8..off + 12].try_into().unwrap());
        let doc_freq = u32::from_le_bytes(self.mmap[off + 12..off + 16].try_into().unwrap());
        Some(TermEntry {
            postings_offset,
            postings_len,
            doc_freq,
        })
    }

    #[inline]
    pub fn get_postings_slice(&self, entry: &TermEntry) -> &[u8] {
        let start = self.postings_offset + entry.postings_offset as usize;
        let end = start + entry.postings_len as usize;
        &self.mmap[start..end]
    }

    pub fn get_term_str(&self, term_idx: usize) -> Option<&str> {
        if term_idx >= self.num_terms as usize {
            return None;
        }
        let off = self.terms_strings_offset + term_idx * 4;
        let s_start = u32::from_le_bytes(self.mmap[off..off + 4].try_into().unwrap()) as usize;
        let s_end = u32::from_le_bytes(self.mmap[off + 4..off + 8].try_into().unwrap()) as usize;
        let data_base = self.terms_strings_offset + (self.num_terms as usize + 1) * 4;
        let start = data_base + s_start;
        let end = data_base + s_end;
        if end > self.mmap.len() {
            return None;
        }
        std::str::from_utf8(&self.mmap[start..end]).ok()
    }
}

pub struct TermDictionary {
    fst_map: Map<memmap2::Mmap>,
}

impl TermDictionary {
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, Box<dyn std::error::Error>> {
        let file = File::open(path)?;
        let mmap = unsafe { memmap2::Mmap::map(&file)? };
        let fst_map = Map::new(mmap)?;
        Ok(Self { fst_map })
    }

    pub fn build_from_index<P: AsRef<Path>>(
        fst_path: P,
        index: &MmapIndex,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let file = File::create(fst_path.as_ref())?;
        let writer = BufWriter::new(file);
        let mut builder = MapBuilder::new(writer)?;
        for idx in 0..index.num_terms as usize {
            if let Some(term) = index.get_term_str(idx) {
                builder.insert(term.as_bytes(), idx as u64)?;
            }
        }
        builder.finish()?;
        Self::open(fst_path)
    }

    #[inline]
    pub fn get(&self, term: &str) -> Option<u64> {
        self.fst_map.get(term.as_bytes())
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.fst_map.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.fst_map.is_empty()
    }

    pub fn suggest_prefix(&self, prefix: &str, limit: usize) -> Vec<String> {
        let prefix_lower = prefix.to_lowercase();
        let prefix_matcher = Str::new(&prefix_lower).starts_with();
        let mut stream = self.fst_map.search(prefix_matcher).into_stream();

        let mut results = Vec::with_capacity(limit);
        while let Some((term_bytes, _val)) = stream.next() {
            if let Ok(term) = std::str::from_utf8(term_bytes) {
                results.push(term.to_string());
                if results.len() >= limit {
                    break;
                }
            }
        }
        results
    }

    pub fn fuzzy_terms(&self, term: &str, max_distance: u32, limit: usize) -> Vec<String> {
        let term_lower = term.to_lowercase();
        let dfa = match Levenshtein::new(&term_lower, max_distance) {
            Ok(d) => d,
            Err(_) => return Vec::new(),
        };

        let mut stream = self.fst_map.search(&dfa).into_stream();
        let mut matched = Vec::with_capacity(limit);

        while let Some((bytes, _val)) = stream.next() {
            if let Ok(s) = std::str::from_utf8(bytes)
                && s != term_lower
            {
                matched.push(s.to_string());
                if matched.len() >= limit {
                    break;
                }
            }
        }
        matched
    }
}
