pub mod api;
pub mod ranking;
pub mod storage;
pub mod tokenizer;

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use api::{create_router, AppState};
use storage::{MmapDocStore, MmapIndex, TermDictionary};
use tokenizer::TokenizerPipeline;

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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new("info"))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let index_path = resolve_path(
        "INDEX_PATH",
        &[
            "../indexer/index.bin",
            "indexer/index.bin",
            "index.bin",
            "/app/index.bin",
        ],
    );
    let docs_path = resolve_path(
        "DOCS_PATH",
        &[
            "../crawler/documents.bin",
            "crawler/documents.bin",
            "documents.bin",
            "/app/documents.bin",
        ],
    );
    let fst_path = resolve_path(
        "FST_PATH",
        &[
            "../indexer/dictionary.fst",
            "indexer/dictionary.fst",
            "dictionary.fst",
            "/app/dictionary.fst",
        ],
    );

    println!("Memory-mapping Inverted Index from {}...", index_path);
    let start_index = Instant::now();
    let index = Arc::new(MmapIndex::open(&index_path)?);
    println!(
        "Index mapped in {:?}: {} docs, {} terms, avg_len={:.2}",
        start_index.elapsed(),
        index.total_docs,
        index.num_terms,
        index.avg_doc_length
    );

    println!("Memory-mapping Document Store from {}...", docs_path);
    let start_docs = Instant::now();
    let doc_store = Arc::new(MmapDocStore::open(&docs_path)?);
    println!(
        "Document store mapped in {:?}: {} docs ready",
        start_docs.elapsed(),
        doc_store.doc_count()
    );

    println!("Opening Term Dictionary FST from {}...", fst_path);
    let start_fst = Instant::now();
    let dictionary = if Path::new(&fst_path).exists() {
        TermDictionary::open(&fst_path)?
    } else {
        println!("FST file not found at {fst_path}. Compiling on the fly from index.bin...");
        TermDictionary::build_from_index(&fst_path, &index)?
    };
    println!(
        "Term Dictionary ready in {:?} ({} terms in lexicon)",
        start_fst.elapsed(),
        dictionary.len()
    );

    let pipeline = TokenizerPipeline::new();
    let state = Arc::new(AppState {
        index,
        doc_store,
        pipeline,
        dictionary: Arc::new(dictionary),
    });

    println!(
        "Engine ready! Serving {} documents across {} terms with 0 RAM deserialization overhead.",
        state.index.total_docs, state.index.num_terms
    );

    let app = create_router(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], 8080));
    println!("Search UI & API running at http://localhost:8080");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
