use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{BufReader, BufWriter, Seek, SeekFrom, Write};
use std::path::Path;
use std::time::Instant;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RawDocument {
    pub id: String,
    pub url: String,
    pub title: String,
    pub content: String,
    pub links: Vec<String>,
}

pub struct DocRecordRef<'a> {
    pub id: &'a str,
    pub url: &'a str,
    pub title: &'a str,
    pub content: &'a str,
    pub links: Vec<&'a str>,
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
        self.file
            .write_all(&(id_bytes.len() as u16).to_le_bytes())?;
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
        let offset = u64::from_le_bytes(
            self.mmap[entry_offset..entry_offset + 8]
                .try_into()
                .unwrap(),
        ) as usize;
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
        let content_len =
            u32::from_le_bytes([slice[pos], slice[pos + 1], slice[pos + 2], slice[pos + 3]])
                as usize;
        pos += 4;
        if pos + content_len > slice.len() {
            return None;
        }
        let content = std::str::from_utf8(&slice[pos..pos + content_len]).ok()?;
        pos += content_len;

        let mut links = Vec::new();
        if pos + 4 <= slice.len() {
            let links_count =
                u32::from_le_bytes([slice[pos], slice[pos + 1], slice[pos + 2], slice[pos + 3]])
                    as usize;
            pos += 4;
            links.reserve(links_count);
            for _ in 0..links_count {
                if pos + 2 > slice.len() {
                    break;
                }
                let link_len = u16::from_le_bytes([slice[pos], slice[pos + 1]]) as usize;
                pos += 2;
                if pos + link_len > slice.len() {
                    break;
                }
                if let Ok(link_str) = std::str::from_utf8(&slice[pos..pos + link_len]) {
                    links.push(link_str);
                }
                pos += link_len;
            }
        }

        Some(DocRecordRef {
            id,
            url,
            title,
            content,
            links,
        })
    }
}

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
    println!(
        "Converted {count} documents to {bin_path} in {:?}",
        start.elapsed()
    );
    Ok(())
}
