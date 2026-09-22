use fastbloom::BloomFilter;
use regex::Regex;
use reqwest::header::{HeaderMap, HeaderValue, USER_AGENT};
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::{Mutex, Semaphore, mpsc};
use tokio::time::sleep;
use url::Url;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Document {
    pub id: String,
    pub url: String,
    pub title: String,
    pub content: String,
    pub links: Vec<String>,
}

pub fn hash_url(raw_url: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(raw_url.as_bytes());
    let result = hasher.finalize();
    hex::encode(&result[..6])
}

pub fn normalize_url(raw_url: &str) -> Option<String> {
    let mut parsed = Url::parse(raw_url).ok()?;
    parsed.set_fragment(None);
    parsed.set_query(None);
    Some(parsed.to_string())
}

// ============================================================================
// Streaming Binary Document Storage Writer (documents.bin)
// ============================================================================

pub struct DocStoreWriter {
    file: BufWriter<File>,
    doc_count: u32,
    offsets: Vec<(u64, u32)>,
    current_offset: u64,
}

impl DocStoreWriter {
    pub fn create<P: AsRef<std::path::Path>>(path: P) -> std::io::Result<Self> {
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let max_pages = 1000;
    let seed_url = "https://en.wikipedia.org/wiki/Search_engine".to_string();

    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let (doc_tx, mut doc_rx) = mpsc::channel::<Document>(100);

    let bloom_filter = Arc::new(Mutex::new(
        BloomFilter::with_false_pos(0.001).expected_items(500_000),
    ));
    let doc_count = Arc::new(AtomicUsize::new(0));
    let semaphore = Arc::new(Semaphore::new(5));

    let disallowed_patterns = vec![
        Regex::new(r"(?i)/w/index\.php").unwrap(),
        Regex::new(r"(?i)/wiki/(Special|Talk|User|Wikipedia|File|MediaWiki|Template|Template_talk|Help|Portal|Category):").unwrap(),
        Regex::new(r"(?i)\?(action|printable|useskin)=").unwrap(),
    ];
    let disallowed = Arc::new(disallowed_patterns);

    let mut headers = HeaderMap::new();
    headers.insert(
        USER_AGENT,
        HeaderValue::from_static(
            "MiniSearchEngineBot/1.0 (+https://github.com/Dercypt/mini-search-engine)",
        ),
    );

    let client = reqwest::Client::builder()
        .default_headers(headers)
        .timeout(Duration::from_secs(10))
        .build()?;

    {
        let mut bloom = bloom_filter.lock().await;
        bloom.insert(&seed_url);
    }
    tx.send(seed_url)?;

    println!(
        "Starting high-performance Rust crawl (Max {} pages)...",
        max_pages
    );

    // Dedicated background writer task: streams docs to disk with low memory footprint
    let writer_handle = tokio::spawn(async move {
        let mut bin_writer = DocStoreWriter::create("documents.bin").expect("failed to create documents.bin");
        let mut json_writer = BufWriter::new(File::create("documents.json").expect("failed to create documents.json"));
        json_writer.write_all(b"[\n").expect("failed to write json header");
        let mut first = true;

        while let Some(doc) = doc_rx.recv().await {
            bin_writer
                .append(&doc.id, &doc.url, &doc.title, &doc.content, &doc.links)
                .expect("failed to append to documents.bin");

            if !first {
                json_writer.write_all(b",\n").expect("failed to write json separator");
            }
            first = false;
            let json = serde_json::to_string(&doc).expect("failed to serialize doc");
            json_writer.write_all(json.as_bytes()).expect("failed to write json doc");
        }

        json_writer.write_all(b"\n]\n").expect("failed to write json footer");
        json_writer.flush().expect("failed to flush json");
        let count = bin_writer.finish().expect("failed to finish documents.bin");
        count
    });

    while let Some(current_url) = rx.recv().await {
        let current_count = doc_count.load(Ordering::Relaxed);
        if current_count >= max_pages {
            break;
        }

        let permit = semaphore.clone().acquire_owned().await.unwrap();
        let client = client.clone();
        let tx = tx.clone();
        let bloom_filter = bloom_filter.clone();
        let doc_tx = doc_tx.clone();
        let doc_count = doc_count.clone();
        let disallowed = disallowed.clone();

        tokio::spawn(async move {
            let _permit = permit;

            sleep(Duration::from_millis(80)).await;

            let response = match client.get(&current_url).send().await {
                Ok(res) => res,
                Err(_) => return,
            };

            let html_text = match response.text().await {
                Ok(text) => text,
                Err(_) => return,
            };

            let (title, content, outgoing_links) = {
                let document = Html::parse_document(&html_text);

                let title_selector = Selector::parse("h1#firstHeading, title").unwrap();
                // Descendant selector for all body text paragraphs
                let body_selector =
                    Selector::parse("div.mw-parser-output p, div#bodyContent p").unwrap();
                // Scope links to the article body content area
                let link_selector = Selector::parse("div#bodyContent a[href]").unwrap();

                let title = document
                    .select(&title_selector)
                    .next()
                    .map(|el| el.text().collect::<Vec<_>>().join(" "))
                    .unwrap_or_default()
                    .trim()
                    .to_string();

                let content_pieces: Vec<String> = document
                    .select(&body_selector)
                    .map(|el| el.text().collect::<Vec<_>>().join(" ").trim().to_string())
                    .filter(|s| s.len() > 20) // Skip empty/stub snippet strings
                    .collect();

                let content = content_pieces.join(" ");

                let mut outgoing_set = HashSet::new();
                let base_url = Url::parse("https://en.wikipedia.org").unwrap();

                for element in document.select(&link_selector) {
                    if let Some(href) = element.value().attr("href") {
                        if let Ok(resolved) = base_url.join(href) {
                            if resolved.host_str() == Some("en.wikipedia.org") {
                                if let Some(normalized) = normalize_url(resolved.as_str()) {
                                    // Skip self-links and unwanted namespace paths
                                    if normalized != current_url
                                        && !disallowed.iter().any(|re| re.is_match(&normalized))
                                    {
                                        outgoing_set.insert(normalized);
                                    }
                                }
                            }
                        }
                    }
                }

                let outgoing_links: Vec<String> = outgoing_set.into_iter().collect();
                (title, content, outgoing_links)
            };

            // Avoid storing empty pages
            if content.is_empty() {
                return;
            }

            let doc = Document {
                id: hash_url(&current_url),
                url: current_url.clone(),
                title,
                content,
                links: outgoing_links.clone(),
            };

            for link in &outgoing_links {
                let mut bloom = bloom_filter.lock().await;
                if !bloom.contains(link) {
                    bloom.insert(link);
                    let _ = tx.send(link.clone());
                }
            }

            let prev = doc_count.fetch_add(1, Ordering::SeqCst);
            if prev < max_pages {
                println!("[{}/{}] Scraped: {}", prev + 1, max_pages, current_url);
                let _ = doc_tx.send(doc).await;
            }
        });
    }

    // Drop original sender so writer knows when all items are received
    drop(doc_tx);
    let total_saved = writer_handle.await?;

    println!(
        "\nDone! Streamed and saved {} documents to documents.bin and documents.json",
        total_saved
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_url_basic() {
        let raw = "https://en.wikipedia.org/wiki/Rust_(programming_language)";
        assert_eq!(
            normalize_url(raw),
            Some("https://en.wikipedia.org/wiki/Rust_(programming_language)".to_string())
        );
    }

    #[test]
    fn test_normalize_url_removes_fragment() {
        let raw = "https://en.wikipedia.org/wiki/Rust#History";
        assert_eq!(
            normalize_url(raw),
            Some("https://en.wikipedia.org/wiki/Rust".to_string())
        );

        let raw_empty_frag = "https://en.wikipedia.org/wiki/Rust#";
        assert_eq!(
            normalize_url(raw_empty_frag),
            Some("https://en.wikipedia.org/wiki/Rust".to_string())
        );
    }

    #[test]
    fn test_normalize_url_removes_query_parameters() {
        let raw = "https://en.wikipedia.org/wiki/Rust?action=edit&section=1";
        assert_eq!(
            normalize_url(raw),
            Some("https://en.wikipedia.org/wiki/Rust".to_string())
        );

        let raw_empty_query = "https://en.wikipedia.org/wiki/Rust?";
        assert_eq!(
            normalize_url(raw_empty_query),
            Some("https://en.wikipedia.org/wiki/Rust".to_string())
        );
    }

    #[test]
    fn test_normalize_url_removes_both_query_and_fragment() {
        let raw = "https://en.wikipedia.org/wiki/Search_engine?source=nav#Architecture";
        assert_eq!(
            normalize_url(raw),
            Some("https://en.wikipedia.org/wiki/Search_engine".to_string())
        );
    }

    #[test]
    fn test_normalize_url_normalizes_scheme_and_host_casing() {
        let raw = "HTTPS://EN.WIKIPEDIA.ORG/wiki/Rust";
        assert_eq!(
            normalize_url(raw),
            Some("https://en.wikipedia.org/wiki/Rust".to_string())
        );
    }

    #[test]
    fn test_normalize_url_preserves_port_and_path() {
        let raw = "http://localhost:8080/search?q=rust#top";
        assert_eq!(
            normalize_url(raw),
            Some("http://localhost:8080/search".to_string())
        );
    }

    #[test]
    fn test_normalize_url_invalid_inputs() {
        assert_eq!(normalize_url(""), None);
        assert_eq!(normalize_url("not a url"), None);
        assert_eq!(normalize_url("/relative/path/only"), None);
        assert_eq!(normalize_url("http://"), None);
        assert_eq!(normalize_url("://bad.url"), None);
    }

    #[test]
    fn test_hash_url_properties() {
        let url1 = "https://en.wikipedia.org/wiki/Rust";
        let url2 = "https://en.wikipedia.org/wiki/Rust";
        let url3 = "https://en.wikipedia.org/wiki/Search_engine";

        let hash1 = hash_url(url1);
        let hash2 = hash_url(url2);
        let hash3 = hash_url(url3);

        // Deterministic
        assert_eq!(hash1, hash2);
        // Distinct for distinct inputs
        assert_ne!(hash1, hash3);
        // 6 bytes in hex = 12 hex characters
        assert_eq!(hash1.len(), 12);
        assert!(hash1.chars().all(|c| c.is_ascii_hexdigit()));
    }
}

