pub mod api;
pub use engine_core::{ranking, storage, tokenizer};

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use api::{AppState, create_router};
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
    let surface_fst_path = resolve_path(
        "SURFACE_FST_PATH",
        &[
            "../indexer/surface.fst",
            "indexer/surface.fst",
            "surface.fst",
            "/app/surface.fst",
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
    let dictionary = Arc::new(if Path::new(&fst_path).exists() {
        TermDictionary::open(&fst_path)?
    } else {
        println!("FST file not found at {fst_path}. Compiling on the fly from index.bin...");
        TermDictionary::build_from_index(&fst_path, &index)?
    });
    println!(
        "Term Dictionary ready in {:?} ({} terms in lexicon)",
        start_fst.elapsed(),
        dictionary.len()
    );

    let pipeline = TokenizerPipeline::new();

    println!(
        "Opening Surface Dictionary FST from {}...",
        surface_fst_path
    );
    let start_surface_fst = Instant::now();
    let surface_dictionary = if Path::new(&surface_fst_path).exists() {
        Arc::new(TermDictionary::open(&surface_fst_path)?)
    } else if doc_store.doc_count() > 0 {
        println!("Surface FST not found at {surface_fst_path}. Compiling from document store...");
        let mut freq_map: std::collections::HashMap<String, u64> = std::collections::HashMap::new();
        for doc_id in 0..doc_store.doc_count() {
            if let Some(doc) = doc_store.get_doc(doc_id) {
                for word in pipeline.extract_surface_words(doc.title) {
                    *freq_map.entry(word).or_insert(0) += 1;
                }
                for word in pipeline.extract_surface_words(doc.content) {
                    *freq_map.entry(word).or_insert(0) += 1;
                }
            }
        }
        Arc::new(TermDictionary::build_from_surface_words(
            &surface_fst_path,
            freq_map,
        )?)
    } else {
        println!(
            "Surface FST not found and document store empty. Falling back to index dictionary..."
        );
        Arc::clone(&dictionary)
    };
    println!(
        "Surface Dictionary ready in {:?} ({} terms in surface lexicon)",
        start_surface_fst.elapsed(),
        surface_dictionary.len()
    );

    let state = Arc::new(AppState {
        index,
        doc_store,
        pipeline,
        dictionary,
        surface_dictionary,
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
