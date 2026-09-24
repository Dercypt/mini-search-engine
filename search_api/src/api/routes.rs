use axum::{Router, routing::get};
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};

use crate::api::handlers::{health_handler, search_handler, suggest_handler, ui_handler};
use crate::api::state::AppState;

pub fn create_router(state: Arc<AppState>) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    Router::new()
        .route("/", get(ui_handler))
        .route("/health", get(health_handler))
        .route("/api/search", get(search_handler))
        .route("/api/suggest", get(suggest_handler))
        .layer(cors)
        .with_state(state)
}
