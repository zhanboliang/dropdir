use std::path::{Component, Path, PathBuf};

use axum::http::StatusCode;

#[derive(Debug)]
pub struct FsError(pub StatusCode, pub String);

impl FsError {
    pub fn forbidden(msg: impl Into<String>) -> Self {
        Self(StatusCode::FORBIDDEN, msg.into())
    }
    pub fn bad_request(msg: impl Into<String>) -> Self {
        Self(StatusCode::BAD_REQUEST, msg.into())
    }
    pub fn not_found(msg: impl Into<String>) -> Self {
        Self(StatusCode::NOT_FOUND, msg.into())
    }
    pub fn io(err: std::io::Error) -> Self {
        let code = match err.kind() {
            std::io::ErrorKind::NotFound => StatusCode::NOT_FOUND,
            std::io::ErrorKind::PermissionDenied => StatusCode::FORBIDDEN,
            std::io::ErrorKind::AlreadyExists => StatusCode::CONFLICT,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        Self(code, err.to_string())
    }
}

impl From<std::io::Error> for FsError {
    fn from(e: std::io::Error) -> Self {
        Self::io(e)
    }
}

/// Safely join a user-supplied relative path onto `root`, rejecting any path
/// that would escape the root (via `..`, absolute paths, null bytes, etc).
///
/// Returns the resolved `PathBuf` (not required to exist).
pub fn safe_join(root: &Path, rel: &str) -> Result<PathBuf, FsError> {
    let trimmed = rel.trim_start_matches('/').trim_start_matches('\\');
    let mut out = root.to_path_buf();

    if trimmed.is_empty() {
        return Ok(out);
    }

    let candidate = Path::new(trimmed);
    if candidate.is_absolute() {
        return Err(FsError::forbidden("absolute path not allowed"));
    }

    for comp in candidate.components() {
        match comp {
            Component::Normal(p) => {
                let s = p
                    .to_str()
                    .ok_or_else(|| FsError::bad_request("non-utf8 path component"))?;
                if s.contains('\0') {
                    return Err(FsError::bad_request("null byte in path"));
                }
                out.push(s);
            }
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(FsError::forbidden("parent traversal not allowed"));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(FsError::forbidden("absolute path not allowed"));
            }
        }
    }

    // Extra defense: walk up to the nearest existing ancestor, canonicalize
    // it, and confirm it's still under root. Catches symlinks that escape
    // the root for both existing and not-yet-created targets.
    let anchor = {
        let mut cur: &Path = &out;
        loop {
            if cur.exists() {
                break cur.to_path_buf();
            }
            match cur.parent() {
                Some(p) => cur = p,
                None => break out.clone(),
            }
        }
    };
    if let Ok(canon) = std::fs::canonicalize(&anchor) {
        if !canon.starts_with(root) {
            return Err(FsError::forbidden("path escapes root"));
        }
    }

    Ok(out)
}

/// Validate a single file/dir name used as the destination of a rename or
/// as an upload filename. Must not contain separators or traversal.
pub fn sanitize_filename(name: &str) -> Result<&str, FsError> {
    if name.is_empty() {
        return Err(FsError::bad_request("empty filename"));
    }
    if name == "." || name == ".." {
        return Err(FsError::bad_request("invalid filename"));
    }
    if name.contains('/') || name.contains('\\') || name.contains('\0') {
        return Err(FsError::bad_request("filename contains separator"));
    }
    Ok(name)
}
