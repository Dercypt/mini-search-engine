use crate::storage::mmap::TermEntry;
use crate::storage::vbyte::{Posting, encode_postings};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::Path;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DocMetadata {
    pub internal_id: u32,
    pub hex_id: String,
    pub url: String,
    pub title: String,
    pub length: u32,
    #[serde(default)]
    pub pagerank: f64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct IndexStore {
    pub total_docs: u32,
    pub avg_doc_length: f64,
    pub doc_meta: HashMap<u32, DocMetadata>,
    pub inverted_index: HashMap<String, Vec<Posting>>,
}

pub struct BinaryIndexWriter;

impl BinaryIndexWriter {
    #[allow(clippy::too_many_arguments)]
    pub fn write_index<P: AsRef<Path>>(
        path: P,
        total_docs: u32,
        avg_doc_length: f64,
        doc_lengths: &[u32],
        pagerank: &[f64],
        doc_metadata: &[DocMetadata],
        sorted_terms: &[String],
        inverted_index: &HashMap<String, Vec<Posting>>,
    ) -> std::io::Result<()> {
        let file = File::create(path)?;
        let mut writer = BufWriter::new(file);

        // Header placeholder (128 bytes)
        let placeholder = [0u8; 128];
        writer.write_all(&placeholder)?;
        let mut current_offset = 128u64;

        // 1. Doc Lengths
        let doc_lengths_offset = current_offset;
        for &len in doc_lengths {
            writer.write_all(&len.to_le_bytes())?;
        }
        current_offset += (doc_lengths.len() * 4) as u64;

        // 2. PageRank
        let pagerank_offset = current_offset;
        for &pr in pagerank {
            writer.write_all(&pr.to_le_bytes())?;
        }
        current_offset += (pagerank.len() * 8) as u64;

        // 3. Doc Meta Index & Data
        let doc_meta_index_offset = current_offset;
        let mut meta_offsets = Vec::with_capacity(doc_metadata.len() + 1);
        let mut meta_data_bytes = Vec::new();

        for meta in doc_metadata {
            let offset_rel = meta_data_bytes.len() as u64;
            meta_offsets.push(offset_rel);

            let hex_bytes = meta.hex_id.as_bytes();
            meta_data_bytes.extend_from_slice(&(hex_bytes.len() as u16).to_le_bytes());
            meta_data_bytes.extend_from_slice(hex_bytes);

            let url_bytes = meta.url.as_bytes();
            meta_data_bytes.extend_from_slice(&(url_bytes.len() as u16).to_le_bytes());
            meta_data_bytes.extend_from_slice(url_bytes);

            let title_bytes = meta.title.as_bytes();
            meta_data_bytes.extend_from_slice(&(title_bytes.len() as u16).to_le_bytes());
            meta_data_bytes.extend_from_slice(title_bytes);
        }
        meta_offsets.push(meta_data_bytes.len() as u64);

        for &off in &meta_offsets {
            writer.write_all(&off.to_le_bytes())?;
        }
        current_offset += (meta_offsets.len() * 8) as u64;

        let doc_meta_data_offset = current_offset;
        writer.write_all(&meta_data_bytes)?;
        current_offset += meta_data_bytes.len() as u64;

        // 3. Postings block (VByte-compressed)
        // First encode all postings so we know offsets and lengths
        let mut postings_bytes = Vec::new();
        let mut term_entries = Vec::with_capacity(sorted_terms.len());

        for term in sorted_terms {
            let postings = inverted_index
                .get(term)
                .map(|v| v.as_slice())
                .unwrap_or(&[]);
            let p_offset = postings_bytes.len() as u64;
            let mut buf = Vec::new();
            encode_postings(postings, &mut buf);
            let p_len = buf.len() as u32;
            postings_bytes.extend_from_slice(&buf);
            term_entries.push(TermEntry {
                postings_offset: p_offset,
                postings_len: p_len,
                doc_freq: postings.len() as u32,
            });
        }

        // 4. Terms Table
        let terms_table_offset = current_offset;
        for entry in &term_entries {
            writer.write_all(&entry.postings_offset.to_le_bytes())?;
            writer.write_all(&entry.postings_len.to_le_bytes())?;
            writer.write_all(&entry.doc_freq.to_le_bytes())?;
        }
        current_offset += (term_entries.len() * 16) as u64;

        // 5. Terms Strings Table
        let terms_strings_offset = current_offset;
        let mut term_offsets = Vec::with_capacity(sorted_terms.len() + 1);
        let mut term_data = Vec::new();
        for term in sorted_terms {
            term_offsets.push(term_data.len() as u32);
            term_data.extend_from_slice(term.as_bytes());
        }
        term_offsets.push(term_data.len() as u32);

        for &off in &term_offsets {
            writer.write_all(&off.to_le_bytes())?;
        }
        current_offset += (term_offsets.len() * 4) as u64;

        writer.write_all(&term_data)?;
        current_offset += term_data.len() as u64;

        // 6. Postings Block
        let postings_offset = current_offset;
        writer.write_all(&postings_bytes)?;
        let postings_bytes_len = postings_bytes.len() as u64;

        writer.flush()?;

        // Seek back to 0 and write final header
        let mut file = writer.into_inner().map_err(|e| e.into_error())?;
        file.seek(SeekFrom::Start(0))?;
        file.write_all(b"MSEIDX01")?;
        file.write_all(&total_docs.to_le_bytes())?;
        file.write_all(&avg_doc_length.to_le_bytes())?;
        file.write_all(&doc_lengths_offset.to_le_bytes())?;
        file.write_all(&doc_meta_index_offset.to_le_bytes())?;
        file.write_all(&doc_meta_data_offset.to_le_bytes())?;
        file.write_all(&terms_table_offset.to_le_bytes())?;
        file.write_all(&(sorted_terms.len() as u32).to_le_bytes())?;
        file.write_all(&terms_strings_offset.to_le_bytes())?;
        file.write_all(&postings_offset.to_le_bytes())?;
        file.write_all(&postings_bytes_len.to_le_bytes())?;
        file.write_all(&pagerank_offset.to_le_bytes())?;
        let padding = [0u8; 40];
        file.write_all(&padding)?;
        file.flush()?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::mmap::MmapIndex;

    #[test]
    fn test_binary_index_pagerank_roundtrip() {
        let test_path = "target/test_pagerank_index.bin";
        let total_docs = 3;
        let avg_doc_length = 50.0;
        let doc_lengths = vec![40, 50, 60];
        let pagerank = vec![0.2, 0.5, 0.3];
        let doc_metadata = vec![
            DocMetadata {
                internal_id: 0,
                hex_id: "000001".to_string(),
                url: "https://example.com/1".to_string(),
                title: "Doc 1".to_string(),
                length: 40,
                pagerank: 0.2,
            },
            DocMetadata {
                internal_id: 1,
                hex_id: "000002".to_string(),
                url: "https://example.com/2".to_string(),
                title: "Doc 2".to_string(),
                length: 50,
                pagerank: 0.5,
            },
            DocMetadata {
                internal_id: 2,
                hex_id: "000003".to_string(),
                url: "https://example.com/3".to_string(),
                title: "Doc 3".to_string(),
                length: 60,
                pagerank: 0.3,
            },
        ];
        let sorted_terms = vec!["term".to_string()];
        let mut inverted_index = HashMap::new();
        inverted_index.insert(
            "term".to_string(),
            vec![Posting {
                doc_id: 0,
                term_frequency: 1,
                positions: vec![0],
            }],
        );

        BinaryIndexWriter::write_index(
            test_path,
            total_docs,
            avg_doc_length,
            &doc_lengths,
            &pagerank,
            &doc_metadata,
            &sorted_terms,
            &inverted_index,
        )
        .expect("write_index failed");

        let mmap_index = MmapIndex::open(test_path).expect("open failed");
        assert_eq!(mmap_index.total_docs, total_docs);
        assert!((mmap_index.get_pagerank(0) - 0.2).abs() < 1e-9);
        assert!((mmap_index.get_pagerank(1) - 0.5).abs() < 1e-9);
        assert!((mmap_index.get_pagerank(2) - 0.3).abs() < 1e-9);

        let meta1 = mmap_index.get_doc_meta(1).expect("meta 1 missing");
        assert!((meta1.pagerank - 0.5).abs() < 1e-9);
        assert_eq!(meta1.title, "Doc 2");

        let _ = std::fs::remove_file(test_path);
    }
}
