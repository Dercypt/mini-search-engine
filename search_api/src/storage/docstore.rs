use std::fs::File;
use std::path::Path;

pub struct DocRecordRef<'a> {
    pub id: &'a str,
    pub url: &'a str,
    pub title: &'a str,
    pub content: &'a str,
    pub links: Vec<&'a str>,
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

    #[inline]
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

    pub fn get_content(&self, doc_id: u32) -> Option<&str> {
        self.get_doc(doc_id).map(|d| d.content)
    }
}
