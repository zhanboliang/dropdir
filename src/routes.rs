use std::path::PathBuf;
use std::sync::Arc;

use axum::{
    body::Body,
    extract::{Multipart, Query, Request, State},
    http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio_util::io::ReaderStream;

use crate::fs_ops::{safe_join, sanitize_filename, FsError};
use crate::text_ext::{is_blocked_for_write, is_editable_text};

pub struct AppState {
    pub root: PathBuf,
    /// Auth token. Empty string = auth disabled (--no-auth).
    pub token: String,
}

pub type SharedState = Arc<AppState>;

const INDEX_HTML: &str = include_str!("assets/index.html");

const MAX_TEXT_BYTES: u64 = 10 * 1024 * 1024; // 10 MB for read/write text

/// Constant-time string compare to avoid timing-leak on token match.
fn secure_eq(a: &str, b: &str) -> bool {
    let ab = a.as_bytes();
    let bb = b.as_bytes();
    if ab.len() != bb.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for i in 0..ab.len() {
        diff |= ab[i] ^ bb[i];
    }
    diff == 0
}

fn token_from_query(query: Option<&str>) -> Option<String> {
    let q = query?;
    for kv in q.split('&') {
        if let Some(raw) = kv.strip_prefix("t=") {
            return Some(percent_decode_lossy(raw));
        }
    }
    None
}

/// Middleware that requires a valid token on every request.
/// Accepts `Authorization: Bearer <t>`, `X-Dropdir-Token: <t>`, or `?t=<t>`.
/// When `state.token` is empty, auth is disabled.
pub async fn auth_middleware(
    State(state): State<SharedState>,
    req: Request,
    next: Next,
) -> Response {
    if state.token.is_empty() {
        return add_security_headers(next.run(req).await);
    }

    let ok = {
        let mut matched = false;
        if let Some(h) = req.headers().get(header::AUTHORIZATION) {
            if let Ok(s) = h.to_str() {
                if let Some(t) = s.strip_prefix("Bearer ") {
                    if secure_eq(t, &state.token) {
                        matched = true;
                    }
                }
            }
        }
        if !matched {
            if let Some(h) = req.headers().get("x-dropdir-token") {
                if let Ok(s) = h.to_str() {
                    if secure_eq(s, &state.token) {
                        matched = true;
                    }
                }
            }
        }
        if !matched {
            if let Some(t) = token_from_query(req.uri().query()) {
                if secure_eq(&t, &state.token) {
                    matched = true;
                }
            }
        }
        matched
    };

    if ok {
        add_security_headers(next.run(req).await)
    } else {
        let mut resp = (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
        resp.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            HeaderValue::from_static("Bearer realm=\"dropdir\""),
        );
        add_security_headers(resp)
    }
}

fn add_security_headers(mut resp: Response) -> Response {
    let h = resp.headers_mut();
    const STATIC_HEADERS: &[(&str, &str)] = &[
        ("x-content-type-options", "nosniff"),
        ("referrer-policy", "no-referrer"),
        ("x-frame-options", "DENY"),
        // Strict CSP: no external resources; inline style/script are part of
        // the bundled index.html only. default-src 'none' blocks everything
        // else, including any inline HTML from uploaded content that might
        // somehow be rendered.
        (
            "content-security-policy",
            "default-src 'none'; style-src 'self' 'unsafe-inline'; \
             script-src 'self' 'unsafe-inline'; connect-src 'self'; \
             img-src 'self' data:; font-src 'self' data:; form-action 'none'; \
             frame-ancestors 'none'; base-uri 'none'",
        ),
    ];
    for (k, v) in STATIC_HEADERS {
        if let (Ok(name), Ok(val)) = (HeaderName::from_bytes(k.as_bytes()), HeaderValue::from_str(v))
        {
            h.entry(name).or_insert(val);
        }
    }
    resp
}

pub async fn index() -> Response {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    (headers, INDEX_HTML).into_response()
}

#[derive(Deserialize)]
pub struct PathQuery {
    #[serde(default)]
    pub path: String,
}

#[derive(Serialize)]
pub struct Entry {
    name: String,
    is_dir: bool,
    size: u64,
    modified: Option<DateTime<Utc>>,
    editable: bool,
}

#[derive(Serialize)]
pub struct ListResponse {
    path: String,
    entries: Vec<Entry>,
}

impl IntoResponse for FsError {
    fn into_response(self) -> Response {
        (self.0, self.1).into_response()
    }
}

pub async fn list(
    State(state): State<SharedState>,
    Query(q): Query<PathQuery>,
) -> Result<Json<ListResponse>, FsError> {
    let target = safe_join(&state.root, &q.path)?;
    let meta = tokio::fs::metadata(&target).await.map_err(FsError::io)?;
    if !meta.is_dir() {
        return Err(FsError::bad_request("not a directory"));
    }

    let mut read_dir = tokio::fs::read_dir(&target).await.map_err(FsError::io)?;
    let mut entries = Vec::new();
    while let Some(ent) = read_dir.next_entry().await.map_err(FsError::io)? {
        let name = match ent.file_name().into_string() {
            Ok(n) => n,
            Err(_) => continue, // skip non-utf8 names
        };
        let md = match ent.metadata().await {
            Ok(m) => m,
            Err(_) => continue,
        };
        let is_dir = md.is_dir();
        let modified = md
            .modified()
            .ok()
            .map(|t| DateTime::<Utc>::from(t));
        let editable = !is_dir && is_editable_text(&ent.path());
        entries.push(Entry {
            name,
            is_dir,
            size: if is_dir { 0 } else { md.len() },
            modified,
            editable,
        });
    }

    entries.sort_by(|a, b| match (a.is_dir, b.is_dir) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.to_ascii_lowercase().cmp(&b.name.to_ascii_lowercase()),
    });

    Ok(Json(ListResponse {
        path: q.path,
        entries,
    }))
}

pub async fn read_file(
    State(state): State<SharedState>,
    Query(q): Query<PathQuery>,
) -> Result<Response, FsError> {
    let target = safe_join(&state.root, &q.path)?;
    let meta = tokio::fs::metadata(&target).await.map_err(FsError::io)?;
    if !meta.is_file() {
        return Err(FsError::bad_request("not a file"));
    }
    if !is_editable_text(&target) {
        return Err(FsError(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "file type not editable".into(),
        ));
    }
    if meta.len() > MAX_TEXT_BYTES {
        return Err(FsError(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("file exceeds {} bytes", MAX_TEXT_BYTES),
        ));
    }
    let bytes = tokio::fs::read(&target).await.map_err(FsError::io)?;
    match String::from_utf8(bytes) {
        Ok(s) => {
            let mut headers = HeaderMap::new();
            headers.insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/plain; charset=utf-8"),
            );
            Ok((headers, s).into_response())
        }
        Err(_) => Err(FsError(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "file is not valid UTF-8".into(),
        )),
    }
}

#[derive(Deserialize)]
pub struct WriteRequest {
    path: String,
    content: String,
}

pub async fn write_file(
    State(state): State<SharedState>,
    Json(req): Json<WriteRequest>,
) -> Result<StatusCode, FsError> {
    let target = safe_join(&state.root, &req.path)?;
    if let Some(name) = target.file_name().and_then(|s| s.to_str()) {
        if is_blocked_for_write(name) {
            return Err(FsError::forbidden(format!(
                "filename '{name}' is blocked for write"
            )));
        }
    } else {
        return Err(FsError::bad_request("invalid filename"));
    }
    if !is_editable_text(&target) {
        return Err(FsError(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "file type not editable".into(),
        ));
    }
    if req.content.len() as u64 > MAX_TEXT_BYTES {
        return Err(FsError(
            StatusCode::PAYLOAD_TOO_LARGE,
            "content too large".into(),
        ));
    }
    if let Some(parent) = target.parent() {
        if !parent.exists() {
            return Err(FsError::not_found("parent directory missing"));
        }
    }
    // Refuse overwrites that would clobber a symlink (the link might point
    // outside the root). If the path exists, require it to be a regular file.
    if let Ok(lmeta) = tokio::fs::symlink_metadata(&target).await {
        if lmeta.file_type().is_symlink() {
            return Err(FsError::forbidden("refusing to write through a symlink"));
        }
        if !lmeta.is_file() {
            return Err(FsError::bad_request("target exists and is not a file"));
        }
    }
    tokio::fs::write(&target, req.content.as_bytes())
        .await
        .map_err(FsError::io)?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn upload(
    State(state): State<SharedState>,
    Query(q): Query<PathQuery>,
    mut multipart: Multipart,
) -> Result<Json<serde_json::Value>, FsError> {
    let dir = safe_join(&state.root, &q.path)?;
    let meta = tokio::fs::metadata(&dir).await.map_err(FsError::io)?;
    if !meta.is_dir() {
        return Err(FsError::bad_request("target is not a directory"));
    }

    let mut saved: Vec<String> = Vec::new();
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| FsError::bad_request(format!("multipart error: {e}")))?
    {
        let original = match field.file_name() {
            Some(n) => n.to_string(),
            None => continue,
        };
        let basename = std::path::Path::new(&original)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("upload.bin")
            .to_string();
        let safe_name = sanitize_filename(&basename)?;
        if is_blocked_for_write(safe_name) {
            return Err(FsError::forbidden(format!(
                "filename '{safe_name}' is blocked for upload"
            )));
        }
        let dest = dir.join(safe_name);
        if let Ok(lmeta) = tokio::fs::symlink_metadata(&dest).await {
            if lmeta.file_type().is_symlink() {
                return Err(FsError::forbidden(
                    "destination is a symlink; refusing to overwrite",
                ));
            }
        }
        let bytes = field
            .bytes()
            .await
            .map_err(|e| FsError::bad_request(format!("read field: {e}")))?;
        tokio::fs::write(&dest, &bytes).await.map_err(FsError::io)?;
        saved.push(basename.clone());
    }

    Ok(Json(serde_json::json!({ "saved": saved })))
}

#[derive(Deserialize)]
pub struct RenameRequest {
    from: String,
    to: String,
}

pub async fn rename(
    State(state): State<SharedState>,
    Json(req): Json<RenameRequest>,
) -> Result<StatusCode, FsError> {
    let from = safe_join(&state.root, &req.from)?;
    let to = safe_join(&state.root, &req.to)?;
    if let Some(name) = to.file_name().and_then(|s| s.to_str()) {
        if is_blocked_for_write(name) {
            return Err(FsError::forbidden(format!(
                "destination filename '{name}' is blocked"
            )));
        }
    } else {
        return Err(FsError::bad_request("invalid destination name"));
    }
    if !from.exists() {
        return Err(FsError::not_found("source not found"));
    }
    if to.exists() {
        return Err(FsError(
            StatusCode::CONFLICT,
            "destination already exists".into(),
        ));
    }
    if let Some(parent) = to.parent() {
        if !parent.exists() {
            return Err(FsError::not_found("destination parent missing"));
        }
    }
    // Don't let a symlinked "from" be used to move a file outside the root.
    if let Ok(lmeta) = tokio::fs::symlink_metadata(&from).await {
        if lmeta.file_type().is_symlink() {
            return Err(FsError::forbidden("source is a symlink; refusing to rename"));
        }
    }
    tokio::fs::rename(&from, &to).await.map_err(FsError::io)?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn delete_entry(
    State(state): State<SharedState>,
    Query(q): Query<PathQuery>,
) -> Result<StatusCode, FsError> {
    let target = safe_join(&state.root, &q.path)?;
    if target == state.root {
        return Err(FsError::forbidden("cannot delete root"));
    }
    let meta = tokio::fs::metadata(&target).await.map_err(FsError::io)?;
    if meta.is_dir() {
        tokio::fs::remove_dir(&target).await.map_err(FsError::io)?;
    } else {
        tokio::fs::remove_file(&target).await.map_err(FsError::io)?;
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn download(
    State(state): State<SharedState>,
    Query(q): Query<PathQuery>,
) -> Result<Response, FsError> {
    let target = safe_join(&state.root, &q.path)?;
    let meta = tokio::fs::metadata(&target).await.map_err(FsError::io)?;
    if !meta.is_file() {
        return Err(FsError::bad_request("not a file"));
    }
    let file = tokio::fs::File::open(&target).await.map_err(FsError::io)?;
    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);

    let filename = target
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("download");
    let mime = mime_guess::from_path(&target)
        .first_or_octet_stream()
        .to_string();

    let disposition = format!("attachment; filename*=UTF-8''{}", percent_encode(filename));
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&mime).unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&disposition).unwrap_or_else(|_| HeaderValue::from_static("attachment")),
    );
    headers.insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&meta.len().to_string()).unwrap(),
    );

    Ok((headers, body).into_response())
}

fn percent_decode_lossy(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'%' && i + 2 < bytes.len() {
            let h = (bytes[i + 1] as char).to_digit(16);
            let l = (bytes[i + 2] as char).to_digit(16);
            if let (Some(h), Some(l)) = (h, l) {
                out.push(((h << 4) | l) as u8);
                i += 3;
                continue;
            }
        }
        if b == b'+' {
            out.push(b' ');
        } else {
            out.push(b);
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        let c = *b;
        let keep = c.is_ascii_alphanumeric()
            || c == b'-'
            || c == b'_'
            || c == b'.'
            || c == b'~';
        if keep {
            out.push(c as char);
        } else {
            out.push_str(&format!("%{:02X}", c));
        }
    }
    out
}
