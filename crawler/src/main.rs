use fastbloom::BloomFilter;
use regex::Regex;
use reqwest::header::{HeaderMap, HeaderValue, USER_AGENT};
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::sync::Arc;
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

fn hash_url(raw_url: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(raw_url.as_bytes());
    let result = hasher.finalize();
    hex::encode(&result[..6])
}

fn normalize_url(raw_url: &str) -> Option<String> {
    let mut parsed = Url::parse(raw_url).ok()?;
    parsed.set_fragment(None);
    parsed.set_query(None);
    Some(parsed.to_string())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let max_pages = 1000;
    let seed_url = "https://en.wikipedia.org/wiki/Search_engine".to_string();

    let (tx, mut rx) = mpsc::unbounded_channel::<String>();

    let bloom_filter = Arc::new(Mutex::new(
        BloomFilter::with_false_pos(0.001).expected_items(10_000),
    ));
    let saved_docs = Arc::new(Mutex::new(Vec::<Document>::new()));
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
            "MiniSearchEngineBot/1.0 (+https://github.com/yourusername/mini-search-engine)",
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

    while let Some(current_url) = rx.recv().await {
        let docs_len = saved_docs.lock().await.len();
        if docs_len >= max_pages {
            break;
        }

        let permit = semaphore.clone().acquire_owned().await.unwrap();
        let client = client.clone();
        let tx = tx.clone();
        let bloom_filter = bloom_filter.clone();
        let saved_docs = saved_docs.clone();
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

            let mut docs = saved_docs.lock().await;
            if docs.len() < max_pages {
                docs.push(doc);
                println!("[{}/{}] Scraped: {}", docs.len(), max_pages, current_url);
            }
        });
    }

    let final_docs = saved_docs.lock().await.clone();
    let json_bytes = serde_json::to_vec_pretty(&final_docs)?;
    std::fs::write("documents.json", json_bytes)?;

    println!(
        "\nDone! Saved {} clean documents to documents.json",
        final_docs.len()
    );
    Ok(())
}
