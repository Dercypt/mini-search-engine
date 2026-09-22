use fst::automaton::{Levenshtein, Str};
use fst::{Automaton, IntoStreamer, Map, MapBuilder, Streamer};
use regex::Regex;
use rust_stemmers::{Algorithm, Stemmer};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufReader, BufWriter, Seek, SeekFrom, Write};
use std::path::Path;
use std::time::Instant;

// ============================================================================
// Data Models
// ============================================================================

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RawDocument {
    pub id: String,
    pub url: String,
    pub title: String,
    pub content: String,
    pub links: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Posting {
    pub doc_id: u32,
    pub term_frequency: u32,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DocMetadata {
    pub internal_id: u32,
    pub hex_id: String,
    pub url: String,
    pub title: String,
    pub length: u32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct IndexStore {
    pub total_docs: u32,
    pub avg_doc_length: f64,
    pub doc_meta: HashMap<u32, DocMetadata>,
    pub inverted_index: HashMap<String, Vec<Posting>>,
}

// ============================================================================
// VByte Compression
// ============================================================================

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
        let byte = bytes[*offset];
        *offset += 1;
        result |= ((byte & 0x7F) as u32) << shift;
        if (byte & 0x80) == 0 {
            return Some(result);
        }
        shift += 7;
        if shift > 35 {
            return None;
        }
    }
    None
}

pub fn encode_postings(postings: &[Posting], buf: &mut Vec<u8>) {
    let mut last_doc_id = 0u32;
    for (i, p) in postings.iter().enumerate() {
        let delta = if i == 0 {
            p.doc_id
        } else {
            p.doc_id.checked_sub(last_doc_id).unwrap_or(0)
        };
        last_doc_id = p.doc_id;
        encode_vbyte(delta, buf);
        encode_vbyte(p.term_frequency, buf);
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
        let term_frequency = decode_vbyte(self.slice, &mut self.offset).unwrap_or(1);
        Some(Posting {
            doc_id,
            term_frequency,
        })
    }
}

// ============================================================================
// Binary Document Storage (documents.bin)
// ============================================================================

pub struct DocRecordRef<'a> {
    pub id: &'a str,
    pub url: &'a str,
    pub title: &'a str,
    pub content: &'a str,
}

pub struct DocStoreWriter {
    file: BufWriter<File>,
    doc_count: u32,
    offsets: Vec<(u64, u32)>,
    current_offset: u64,
}

impl DocStoreWriter {
    pub fn create<P: AsRef<Path>>(path: P) -> std::io::Result<Self> {
        let file = File::create(path)?;
        let mut writer = BufWriter::new(file);
        let placeholder = [0u8; 64];
        writer.write_all(&placeholder)?;
        Ok(Self {
            file: writer,
            doc_count: 0,
            offsets: Vec::new(),
            current_offset: 64,
        })
    }

    pub fn append(
        &mut self,
        id: &str,
        url: &str,
        title: &str,
        content: &str,
        links: &[String],
    ) -> std::io::Result<u32> {
        let start_offset = self.current_offset;
        let mut written = 0u32;

        let id_bytes = id.as_bytes();
        self.file.write_all(&(id_bytes.len() as u16).to_le_bytes())?;
        self.file.write_all(id_bytes)?;
        written += 2 + id_bytes.len() as u32;

        let url_bytes = url.as_bytes();
        self.file
            .write_all(&(url_bytes.len() as u16).to_le_bytes())?;
        self.file.write_all(url_bytes)?;
        written += 2 + url_bytes.len() as u32;

        let title_bytes = title.as_bytes();
        self.file
            .write_all(&(title_bytes.len() as u16).to_le_bytes())?;
        self.file.write_all(title_bytes)?;
        written += 2 + title_bytes.len() as u32;

        let content_bytes = content.as_bytes();
        self.file
            .write_all(&(content_bytes.len() as u32).to_le_bytes())?;
        self.file.write_all(content_bytes)?;
        written += 4 + content_bytes.len() as u32;

        self.file.write_all(&(links.len() as u32).to_le_bytes())?;
        written += 4;
        for link in links {
            let link_bytes = link.as_bytes();
            self.file
                .write_all(&(link_bytes.len() as u16).to_le_bytes())?;
            self.file.write_all(link_bytes)?;
            written += 2 + link_bytes.len() as u32;
        }

        self.offsets.push((start_offset, written));
        self.current_offset += written as u64;
        let doc_id = self.doc_count;
        self.doc_count += 1;
        Ok(doc_id)
    }

    pub fn finish(mut self) -> std::io::Result<u32> {
        let index_offset = self.current_offset;
        for (offset, len) in &self.offsets {
            self.file.write_all(&offset.to_le_bytes())?;
            self.file.write_all(&len.to_le_bytes())?;
        }
        self.file.flush()?;

        let mut file = self.file.into_inner().map_err(|e| e.into_error())?;
        file.seek(SeekFrom::Start(0))?;
        file.write_all(b"MSEDOC01")?;
        file.write_all(&self.doc_count.to_le_bytes())?;
        file.write_all(&index_offset.to_le_bytes())?;
        let padding = [0u8; 44];
        file.write_all(&padding)?;
        file.flush()?;
        Ok(self.doc_count)
    }
}

pub struct MmapDocStore {
    mmap: memmap2::Mmap,
    doc_count: u32,
    index_offset: usize,
}

impl MmapDocStore {
    pub fn open<P: AsRef<Path>>(path: P) -> std::io::Result<Self> {
        let file = File::open(path)?;
        let mmap = unsafe { memmap2::Mmap::map(&file)? };
        if mmap.len() < 64 || &mmap[0..8] != b"MSEDOC01" {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Invalid document store magic header",
            ));
        }
        let doc_count = u32::from_le_bytes(mmap[8..12].try_into().unwrap());
        let index_offset = u64::from_le_bytes(mmap[12..20].try_into().unwrap()) as usize;
        Ok(Self {
            mmap,
            doc_count,
            index_offset,
        })
    }

    pub fn doc_count(&self) -> u32 {
        self.doc_count
    }

    pub fn get_doc_slice(&self, doc_id: u32) -> Option<&[u8]> {
        if doc_id >= self.doc_count {
            return None;
        }
        let entry_offset = self.index_offset + (doc_id as usize) * 12;
        if entry_offset + 12 > self.mmap.len() {
            return None;
        }
        let offset =
            u64::from_le_bytes(self.mmap[entry_offset..entry_offset + 8].try_into().unwrap())
                as usize;
        let len = u32::from_le_bytes(
            self.mmap[entry_offset + 8..entry_offset + 12]
                .try_into()
                .unwrap(),
        ) as usize;
        if offset + len > self.mmap.len() {
            return None;
        }
        Some(&self.mmap[offset..offset + len])
    }

    pub fn get_doc(&self, doc_id: u32) -> Option<DocRecordRef<'_>> {
        let slice = self.get_doc_slice(doc_id)?;
        let mut pos = 0;
        if slice.len() < 2 {
            return None;
        }
        let id_len = u16::from_le_bytes([slice[pos], slice[pos + 1]]) as usize;
        pos += 2;
        if pos + id_len > slice.len() {
            return None;
        }
        let id = std::str::from_utf8(&slice[pos..pos + id_len]).ok()?;
        pos += id_len;

        if pos + 2 > slice.len() {
            return None;
        }
        let url_len = u16::from_le_bytes([slice[pos], slice[pos + 1]]) as usize;
        pos += 2;
        if pos + url_len > slice.len() {
            return None;
        }
        let url = std::str::from_utf8(&slice[pos..pos + url_len]).ok()?;
        pos += url_len;

        if pos + 2 > slice.len() {
            return None;
        }
        let title_len = u16::from_le_bytes([slice[pos], slice[pos + 1]]) as usize;
        pos += 2;
        if pos + title_len > slice.len() {
            return None;
        }
        let title = std::str::from_utf8(&slice[pos..pos + title_len]).ok()?;
        pos += title_len;

        if pos + 4 > slice.len() {
            return None;
        }
        let content_len = u32::from_le_bytes([
            slice[pos],
            slice[pos + 1],
            slice[pos + 2],
            slice[pos + 3],
        ]) as usize;
        pos += 4;
        if pos + content_len > slice.len() {
            return None;
        }
        let content = std::str::from_utf8(&slice[pos..pos + content_len]).ok()?;

        Some(DocRecordRef {
            id,
            url,
            title,
            content,
        })
    }
}

// Convert JSON documents to binary document store if needed
pub fn convert_json_to_bin_if_needed(json_path: &str, bin_path: &str) -> std::io::Result<()> {
    if Path::new(bin_path).exists() {
        return Ok(());
    }
    if !Path::new(json_path).exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("Neither {bin_path} nor {json_path} found"),
        ));
    }
    println!("Converting {json_path} to {bin_path} (binary document storage)...");
    let start = Instant::now();
    let file = File::open(json_path)?;
    let reader = BufReader::new(file);
    let docs: Vec<RawDocument> = serde_json::from_reader(reader)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    let mut writer = DocStoreWriter::create(bin_path)?;
    for doc in &docs {
        writer.append(&doc.id, &doc.url, &doc.title, &doc.content, &doc.links)?;
    }
    let count = writer.finish()?;
    println!("Converted {count} documents to {bin_path} in {:?}", start.elapsed());
    Ok(())
}

// ============================================================================
// Binary Inverted Index (index.bin)
// ============================================================================

pub struct TermEntry {
    pub postings_offset: u64,
    pub postings_len: u32,
    pub doc_freq: u32,
}

pub struct DocMetaRef<'a> {
    pub hex_id: &'a str,
    pub url: &'a str,
    pub title: &'a str,
}

pub struct BinaryIndexWriter;

impl BinaryIndexWriter {
    pub fn write_index<P: AsRef<Path>>(
        path: P,
        total_docs: u32,
        avg_doc_length: f64,
        doc_lengths: &[u32],
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

        // 2. Doc Meta Index & Data
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
            let postings = inverted_index.get(term).map(|v| v.as_slice()).unwrap_or(&[]);
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
        let padding = [0u8; 48];
        file.write_all(&padding)?;
        file.flush()?;

        Ok(())
    }
}

pub struct MmapIndex {
    mmap: memmap2::Mmap,
    pub total_docs: u32,
    pub avg_doc_length: f64,
    pub num_terms: u32,
    doc_lengths_offset: usize,
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

        Ok(Self {
            mmap,
            total_docs,
            avg_doc_length,
            num_terms,
            doc_lengths_offset,
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

        Some(DocMetaRef {
            hex_id,
            url,
            title,
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

// ============================================================================
// Term Dictionary FST (using fst::Map)
// ============================================================================

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

    pub fn get(&self, term: &str) -> Option<u64> {
        self.fst_map.get(term.as_bytes())
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
            if let Ok(s) = std::str::from_utf8(bytes) {
                if s != term_lower {
                    matched.push(s.to_string());
                    if matched.len() >= limit {
                        break;
                    }
                }
            }
        }
        matched
    }
}

// ============================================================================
// Tokenizer Pipeline
// ============================================================================

pub struct TokenizerPipeline {
    stemmer: Stemmer,
    stop_words: HashSet<&'static str>,
    regex: Regex,
}

impl TokenizerPipeline {
    pub fn new() -> Self {
        let stop_words: HashSet<&'static str> = [
            "a", "about", "above", "after", "again", "against", "all", "am", "an", "and", "any",
            "are", "aren't", "as", "at", "be", "because", "been", "before", "being", "below",
            "between", "both", "but", "by", "can't", "cannot", "could", "couldn't", "did",
            "didn't", "do", "does", "doesn't", "doing", "don't", "down", "during", "each",
            "few", "for", "from", "further", "had", "hadn't", "has", "hasn't", "have",
            "haven't", "having", "he", "he'd", "he'll", "he's", "her", "here", "here's", "hers",
            "herself", "him", "himself", "his", "how", "how's", "i", "i'd", "i'll", "i'm",
            "i've", "if", "in", "into", "is", "isn't", "it", "it's", "its", "itself", "let's",
            "me", "more", "most", "mustn't", "my", "myself", "no", "nor", "not", "of", "off",
            "on", "once", "only", "or", "other", "ought", "our", "ours", "ourselves", "out",
            "over", "own", "same", "shan't", "she", "she'd", "she'll", "she's", "should",
            "shouldn't", "so", "some", "such", "than", "that", "that's", "the", "their",
            "theirs", "them", "themselves", "then", "there", "there's", "these", "they",
            "they'd", "they'll", "they're", "they've", "this", "those", "through", "to", "too",
            "under", "until", "up", "very", "was", "wasn't", "we", "we'd", "we'll", "we're",
            "we've", "were", "weren't", "what", "what's", "when", "when's", "where", "where's",
            "which", "while", "who", "who's", "whom", "why", "why's", "with", "won't", "would",
            "wouldn't", "you", "you'd", "you'll", "you're", "you've", "your", "yours",
            "yourself", "yourselves",
        ]
        .into_iter()
        .collect();

        Self {
            stemmer: Stemmer::create(Algorithm::English),
            stop_words,
            regex: Regex::new(r"[^a-zA-Z0-9\s]+").unwrap(),
        }
    }

    pub fn tokenize(&self, text: &str) -> Vec<String> {
        let cleaned = self.regex.replace_all(text, " ").to_lowercase();
        cleaned
            .split_whitespace()
            .filter(|token| token.len() > 1 && !self.stop_words.contains(token))
            .map(|token| self.stemmer.stem(token).to_string())
            .collect()
    }
}

// ============================================================================
// Main & BM25 Scoring
// ============================================================================

fn resolve_path(env_var: &str, candidates: &[&str]) -> String {
    if let Ok(val) = std::env::var(env_var) {
        return val;
    }
    for candidate in candidates {
        if Path::new(candidate).exists() {
            return candidate.to_string();
        }
    }
    candidates[0].to_string()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let docs_json_path = resolve_path(
        "DOCS_JSON_PATH",
        &[
            "../crawler/documents.json",
            "crawler/documents.json",
            "documents.json",
        ],
    );
    let docs_bin_path = resolve_path(
        "DOCS_PATH",
        &[
            "../crawler/documents.bin",
            "crawler/documents.bin",
            "documents.bin",
        ],
    );
    let output_bin_index_path = resolve_path("INDEX_PATH", &["index.bin", "indexer/index.bin"]);
    let output_json_index_path =
        resolve_path("INDEX_JSON_PATH", &["index.json", "indexer/index.json"]);
    let output_fst_path = resolve_path("FST_PATH", &["dictionary.fst", "indexer/dictionary.fst"]);

    // Auto-convert JSON docs to binary storage if needed
    if !Path::new(&docs_bin_path).exists() && Path::new(&docs_json_path).exists() {
        convert_json_to_bin_if_needed(&docs_json_path, &docs_bin_path)?;
    }

    let pipeline = TokenizerPipeline::new();

    println!("Opening document store from {}...", docs_bin_path);
    let start_time = Instant::now();
    let doc_store = MmapDocStore::open(&docs_bin_path)?;
    let total_docs = doc_store.doc_count();
    println!(
        "Memory-mapped {} documents in {:?}",
        total_docs,
        start_time.elapsed()
    );

    // Build inverted index and metadata from mmap document store
    println!("Indexing documents...");
    let index_start = Instant::now();
    let mut inverted_index: HashMap<String, Vec<Posting>> = HashMap::new();
    let mut doc_metadata: Vec<DocMetadata> = Vec::with_capacity(total_docs as usize);
    let mut doc_lengths: Vec<u32> = Vec::with_capacity(total_docs as usize);
    let mut total_terms: usize = 0;

    for doc_id in 0..total_docs {
        let doc = doc_store
            .get_doc(doc_id)
            .ok_or_else(|| format!("Failed to read doc {doc_id}"))?;

        // Title terms receive double weight
        let mut title_terms = pipeline.tokenize(doc.title);
        let content_terms = pipeline.tokenize(doc.content);

        let mut all_terms = Vec::with_capacity(title_terms.len() * 2 + content_terms.len());
        all_terms.append(&mut title_terms.clone());
        all_terms.append(&mut title_terms);
        all_terms.extend(content_terms);

        let doc_length = all_terms.len() as u32;
        total_terms += doc_length as usize;
        doc_lengths.push(doc_length);

        doc_metadata.push(DocMetadata {
            internal_id: doc_id,
            hex_id: doc.id.to_string(),
            url: doc.url.to_string(),
            title: doc.title.to_string(),
            length: doc_length,
        });

        let mut tf_map: HashMap<String, u32> = HashMap::new();
        for term in all_terms {
            *tf_map.entry(term).or_insert(0) += 1;
        }

        for (term, tf) in tf_map {
            inverted_index
                .entry(term)
                .or_default()
                .push(Posting {
                    doc_id,
                    term_frequency: tf,
                });
        }
    }

    let avg_doc_length = if total_docs > 0 {
        total_terms as f64 / total_docs as f64
    } else {
        0.0
    };

    println!(
        "Indexed {} documents ({} terms) in {:?}",
        total_docs,
        total_terms,
        index_start.elapsed()
    );

    // Sort terms lexicographically for FST and binary index
    let mut sorted_terms: Vec<String> = inverted_index.keys().cloned().collect();
    sorted_terms.sort();
    sorted_terms.dedup();

    println!("\n--- Indexing Statistics ---");
    println!("Total Documents: {}", total_docs);
    println!("Unique Vocabulary Terms: {}", sorted_terms.len());
    println!("Average Document Length: {:.2} terms", avg_doc_length);

    // Save binary memory-mapped index with VByte-compressed postings
    let bin_save_start = Instant::now();
    BinaryIndexWriter::write_index(
        &output_bin_index_path,
        total_docs,
        avg_doc_length,
        &doc_lengths,
        &doc_metadata,
        &sorted_terms,
        &inverted_index,
    )?;
    println!(
        "Saved binary mmap index to {} with VByte postings in {:?}",
        output_bin_index_path,
        bin_save_start.elapsed()
    );

    // Build and save Lexicon FST (Map mapping term -> term_id)
    let fst_start = Instant::now();
    let fst_file = File::create(&output_fst_path)?;
    let fst_writer = BufWriter::new(fst_file);
    let mut builder = MapBuilder::new(fst_writer)?;
    for (term_idx, term) in sorted_terms.iter().enumerate() {
        builder.insert(term.as_bytes(), term_idx as u64)?;
    }
    builder.finish()?;
    println!(
        "Compiled and saved {} terms to {} in {:?}",
        sorted_terms.len(),
        output_fst_path,
        fst_start.elapsed()
    );

    // Save legacy index.json for backward compatibility
    let json_save_start = Instant::now();
    let mut doc_meta_map: HashMap<u32, DocMetadata> = HashMap::with_capacity(doc_metadata.len());
    for meta in doc_metadata {
        doc_meta_map.insert(meta.internal_id, meta);
    }
    let index_store = IndexStore {
        total_docs,
        avg_doc_length,
        doc_meta: doc_meta_map,
        inverted_index,
    };
    let json_bytes = serde_json::to_vec_pretty(&index_store)?;
    let mut out_file = File::create(&output_json_index_path)?;
    out_file.write_all(&json_bytes)?;
    println!(
        "Saved backward-compatible {} in {:?}",
        output_json_index_path,
        json_save_start.elapsed()
    );

    // Test BM25 Query Evaluation using memory-mapped binary index
    println!("\nVerifying retrieval against memory-mapped binary index (mmap)...");
    let mmap_index = MmapIndex::open(&output_bin_index_path)?;
    let dictionary = TermDictionary::open(&output_fst_path)?;

    let test_queries = ["search engine", "page rank algorithm", "open source"];
    for query in test_queries {
        println!("\n--- Test Query: \"{}\" ---", query);
        let results = search_bm25_mmap(query, &mmap_index, &dictionary, &pipeline, 5);
        for (rank, (doc, score)) in results.iter().enumerate() {
            println!("{}. [{:.4}] {} ({})", rank + 1, score, doc.title, doc.url);
        }
    }

    Ok(())
}

pub fn search_bm25_mmap<'a>(
    query: &str,
    index: &'a MmapIndex,
    dictionary: &TermDictionary,
    pipeline: &TokenizerPipeline,
    top_k: usize,
) -> Vec<(DocMetaRef<'a>, f64)> {
    let k1 = 1.5;
    let b = 0.75;
    let query_terms = pipeline.tokenize(query);
    if query_terms.is_empty() {
        return Vec::new();
    }

    let mut scores: HashMap<u32, f64> = HashMap::new();
    let n = index.total_docs as f64;

    for term in &query_terms {
        if let Some(term_idx) = dictionary.get(term) {
            if let Some(entry) = index.get_term_entry(term_idx as usize) {
                let n_q = entry.doc_freq as f64;
                let idf = ((n - n_q + 0.5) / (n_q + 0.5) + 1.0).ln();
                let postings_slice = index.get_postings_slice(&entry);
                let iter = PostingsIterator::new(postings_slice, entry.doc_freq as usize);

                for posting in iter {
                    let doc_len = index.get_doc_length(posting.doc_id) as f64;
                    let tf = posting.term_frequency as f64;
                    let num = tf * (k1 + 1.0);
                    let denom = tf + k1 * (1.0 - b + b * (doc_len / index.avg_doc_length));
                    let term_score = idf * (num / denom);

                    *scores.entry(posting.doc_id).or_insert(0.0) += term_score;
                }
            }
        }
    }

    let mut ranked: Vec<(u32, f64)> = scores.into_iter().collect();
    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    ranked.truncate(top_k);

    ranked
        .into_iter()
        .filter_map(|(doc_id, score)| index.get_doc_meta(doc_id).map(|meta| (meta, score)))
        .collect()
}
