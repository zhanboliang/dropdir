use std::path::Path;

const TEXT_EXTS: &[&str] = &[
    "txt", "md", "markdown", "log", "csv", "tsv",
    "json", "yaml", "yml", "toml", "ini", "cfg", "conf", "env",
    "xml", "html", "htm", "css", "scss", "sass", "less",
    "js", "mjs", "cjs", "ts", "tsx", "jsx", "vue", "svelte",
    "rs", "go", "py", "rb", "java", "kt", "kts", "scala",
    "c", "h", "cpp", "hpp", "cc", "hh", "cs", "php", "lua", "pl",
    "sql", "graphql", "gql", "proto",
    "gradle", "lock", "properties", "svg",
    "gitignore", "gitattributes", "editorconfig",
    "dockerfile", "makefile",
];

const TEXT_FILENAMES: &[&str] = &[
    "dockerfile", "makefile", "license", "readme",
    "authors", "contributors", "changelog", "copying",
    ".gitignore", ".gitattributes", ".editorconfig",
    ".env", ".dockerignore",
];

/// Extensions refused by write-side operations (upload, write, rename-to).
/// These are native-executable or shell-launch formats — a network peer
/// using dropdir shouldn't be able to plant new copies of them on the host.
/// Download/read of existing files is NOT blocked; this is a write-side wall.
const BLOCKED_EXTS: &[&str] = &[
    // Windows native
    "exe", "dll", "msi", "bat", "cmd", "ps1", "ps2", "psm1",
    "vbs", "vbe", "scr", "com", "cpl", "hta", "lnk",
    // macOS native
    "app", "dmg", "pkg", "kext", "mpkg",
    // Unix shared libs / binaries
    "so", "dylib", "elf",
    // Shell scripts (common shebang targets)
    "sh", "bash", "zsh", "fish", "ksh", "csh", "tcsh",
    // Android / Java deploy
    "apk", "aab", "jar", "war", "ear",
    // Misc native
    "wsf", "wsh", "jse", "reg",
];

/// Filenames (full, case-insensitive) that are always rejected on write.
const BLOCKED_NAMES: &[&str] = &[
    "autorun.inf",
    ".bashrc", ".bash_profile", ".bash_login", ".bash_logout",
    ".zshrc", ".zprofile", ".zlogin",
    ".profile", ".login", ".cshrc",
    "authorized_keys", "known_hosts",
    ".rhosts", ".forward", ".netrc",
];

pub fn is_editable_text(path: &Path) -> bool {
    if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
        if TEXT_EXTS.iter().any(|e| e.eq_ignore_ascii_case(ext)) {
            return true;
        }
    }
    if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
        let lower = name.to_ascii_lowercase();
        if TEXT_FILENAMES.iter().any(|n| *n == lower) {
            return true;
        }
    }
    false
}

/// True if this filename is on the write-side blocklist.
/// `name` should be the LAST path component (no directories).
pub fn is_blocked_for_write(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    if BLOCKED_NAMES.iter().any(|n| *n == lower) {
        return true;
    }
    if let Some(dot) = lower.rfind('.') {
        let ext = &lower[dot + 1..];
        if BLOCKED_EXTS.iter().any(|e| *e == ext) {
            return true;
        }
    }
    false
}
