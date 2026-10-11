//! HTTP/JSON API consumed by the native client. See ARCHITECTURE.md for the contract.
//!
//! Database calls are short, local SQLite operations, so handlers call the
//! (synchronous) `Db` directly instead of hopping to a blocking thread pool.

use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{FromRequest, FromRequestParts, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tower_http::trace::TraceLayer;

use crate::db::{
    Account, Compose, Db, DbError, FolderCount, Message, MessagePatch, MessageSummary, SendResult,
};

#[derive(Debug)]
pub enum AppError {
    Db(DbError),
    BadRequest(String),
}

impl From<DbError> for AppError {
    fn from(e: DbError) -> Self {
        AppError::Db(e)
    }
}

impl From<JsonRejection> for AppError {
    fn from(e: JsonRejection) -> Self {
        AppError::BadRequest(e.body_text())
    }
}

impl From<PathRejection> for AppError {
    fn from(e: PathRejection) -> Self {
        AppError::BadRequest(e.body_text())
    }
}

impl From<QueryRejection> for AppError {
    fn from(e: QueryRejection) -> Self {
        AppError::BadRequest(e.body_text())
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            AppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            AppError::Db(e) => {
                let status = match &e {
                    DbError::NotFound(_) => StatusCode::NOT_FOUND,
                    DbError::Invalid(_) => StatusCode::BAD_REQUEST,
                    DbError::Conflict(_) => StatusCode::CONFLICT,
                    DbError::Sql(err) => {
                        tracing::error!("database error: {err}");
                        StatusCode::INTERNAL_SERVER_ERROR
                    }
                };
                (status, e.to_string())
            }
        };
        (status, Json(json!({ "error": message }))).into_response()
    }
}

/// `Json` extractor whose rejections use the API's `{"error": ...}` format.
#[derive(FromRequest)]
#[from_request(via(axum::Json), rejection(AppError))]
struct AppJson<T>(T);

#[derive(FromRequestParts)]
#[from_request(via(axum::extract::Path), rejection(AppError))]
struct AppPath<T>(T);

#[derive(FromRequestParts)]
#[from_request(via(axum::extract::Query), rejection(AppError))]
struct AppQuery<T>(T);

type ApiResult<T> = Result<T, AppError>;

pub fn router(db: Db) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/accounts", get(list_accounts).post(create_account))
        .route("/api/accounts/{id}", axum::routing::delete(delete_account))
        .route("/api/accounts/{id}/folders", get(folder_counts))
        .route(
            "/api/accounts/{id}/folders/{folder}/read",
            post(mark_folder_read),
        )
        .route("/api/accounts/{id}/trash/empty", post(empty_trash))
        .route("/api/messages", get(list_messages))
        .route(
            "/api/messages/{id}",
            get(get_message)
                .patch(update_message)
                .delete(delete_message),
        )
        .route("/api/send", post(send))
        .route("/api/drafts", post(create_draft))
        .route("/api/drafts/{id}", put(update_draft))
        .layer(TraceLayer::new_for_http())
        .with_state(db)
}

async fn health() -> Json<serde_json::Value> {
    Json(json!({ "status": "ok" }))
}

async fn list_accounts(State(db): State<Db>) -> ApiResult<Json<Vec<Account>>> {
    Ok(Json(db.list_accounts()?))
}

#[derive(Deserialize)]
struct NewAccount {
    name: String,
    email: String,
}

async fn create_account(
    State(db): State<Db>,
    AppJson(body): AppJson<NewAccount>,
) -> ApiResult<(StatusCode, Json<Account>)> {
    let account = db.create_account(&body.name, &body.email)?;
    Ok((StatusCode::CREATED, Json(account)))
}

async fn delete_account(State(db): State<Db>, AppPath(id): AppPath<i64>) -> ApiResult<StatusCode> {
    db.delete_account(id)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn folder_counts(
    State(db): State<Db>,
    AppPath(id): AppPath<i64>,
) -> ApiResult<Json<Vec<FolderCount>>> {
    Ok(Json(db.folder_counts(id)?))
}

#[derive(Serialize)]
struct Updated {
    updated: usize,
}

async fn mark_folder_read(
    State(db): State<Db>,
    AppPath((id, folder)): AppPath<(i64, String)>,
) -> ApiResult<Json<Updated>> {
    Ok(Json(Updated {
        updated: db.mark_all_read(id, &folder)?,
    }))
}

#[derive(Serialize)]
struct Deleted {
    deleted: usize,
}

async fn empty_trash(State(db): State<Db>, AppPath(id): AppPath<i64>) -> ApiResult<Json<Deleted>> {
    Ok(Json(Deleted {
        deleted: db.empty_trash(id)?,
    }))
}

#[derive(Deserialize)]
struct ListQuery {
    account_id: i64,
    folder: Option<String>,
    q: Option<String>,
}

async fn list_messages(
    State(db): State<Db>,
    AppQuery(query): AppQuery<ListQuery>,
) -> ApiResult<Json<Vec<MessageSummary>>> {
    db.get_account(query.account_id)?;
    let folder = query.folder.as_deref().unwrap_or("inbox");
    Ok(Json(db.list_messages(
        query.account_id,
        folder,
        query.q.as_deref(),
    )?))
}

async fn get_message(State(db): State<Db>, AppPath(id): AppPath<i64>) -> ApiResult<Json<Message>> {
    Ok(Json(db.get_message(id)?))
}

async fn update_message(
    State(db): State<Db>,
    AppPath(id): AppPath<i64>,
    AppJson(patch): AppJson<MessagePatch>,
) -> ApiResult<Json<Message>> {
    Ok(Json(db.update_message(id, &patch)?))
}

#[derive(Serialize)]
struct DeleteResult {
    permanent: bool,
}

async fn delete_message(
    State(db): State<Db>,
    AppPath(id): AppPath<i64>,
) -> ApiResult<Json<DeleteResult>> {
    Ok(Json(DeleteResult {
        permanent: db.delete_message(id)?,
    }))
}

async fn send(
    State(db): State<Db>,
    AppJson(compose): AppJson<Compose>,
) -> ApiResult<(StatusCode, Json<SendResult>)> {
    Ok((StatusCode::CREATED, Json(db.send(&compose)?)))
}

async fn create_draft(
    State(db): State<Db>,
    AppJson(compose): AppJson<Compose>,
) -> ApiResult<(StatusCode, Json<Message>)> {
    Ok((StatusCode::CREATED, Json(db.save_draft(None, &compose)?)))
}

async fn update_draft(
    State(db): State<Db>,
    AppPath(id): AppPath<i64>,
    AppJson(compose): AppJson<Compose>,
) -> ApiResult<Json<Message>> {
    Ok(Json(db.save_draft(Some(id), &compose)?))
}
