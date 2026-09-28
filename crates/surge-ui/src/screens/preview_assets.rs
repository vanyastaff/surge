//! Capability-scoped static assets for the native application preview.
use std::{io::Read, path::PathBuf};

pub(super) struct PreviewAssets {
    root: cap_std::fs::Dir,
}
pub(super) struct PreviewAsset {
    pub(super) bytes: Vec<u8>,
    pub(super) mime: &'static str,
}
#[derive(Debug)]
pub(super) struct AssetError {
    pub(super) status: u16,
    pub(super) message: &'static str,
}

impl PreviewAssets {
    pub(super) fn new(root: PathBuf) -> Result<Self, String> {
        cap_std::fs::Dir::open_ambient_dir(root, cap_std::ambient_authority())
            .map(|root| Self { root })
            .map_err(|error| error.to_string())
    }
    pub(super) fn load(&self, encoded_path: &str) -> Result<PreviewAsset, AssetError> {
        let path = asset_path(encoded_path)?;
        let mime = asset_mime(&path).ok_or(AssetError {
            status: 415,
            message: "Unsupported asset type",
        })?;
        let mut options = cap_std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use cap_std::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NONBLOCK);
        }
        let file = self
            .root
            .open_with(&path, &options)
            .map_err(|_| AssetError {
                status: 404,
                message: "Not found",
            })?;
        let metadata = file.metadata().map_err(|_| AssetError {
            status: 404,
            message: "Not found",
        })?;
        if !metadata.is_file() {
            return Err(AssetError {
                status: 404,
                message: "Not found",
            });
        }
        const MAX_BYTES: u64 = 8 * 1024 * 1024;
        if metadata.len() > MAX_BYTES {
            return Err(AssetError {
                status: 413,
                message: "Asset too large",
            });
        }
        let mut bytes = Vec::new();
        file.take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| AssetError {
                status: 500,
                message: "Asset read failed",
            })?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(AssetError {
                status: 413,
                message: "Asset too large",
            });
        }
        Ok(PreviewAsset { bytes, mime })
    }
}

fn asset_path(encoded: &str) -> Result<String, AssetError> {
    let invalid = || AssetError {
        status: 400,
        message: "Invalid asset path",
    };
    let raw = encoded.as_bytes();
    for (index, byte) in raw.iter().enumerate() {
        if *byte == b'%'
            && (raw.get(index + 1).is_none_or(|b| !b.is_ascii_hexdigit())
                || raw.get(index + 2).is_none_or(|b| !b.is_ascii_hexdigit()))
        {
            return Err(invalid());
        }
    }
    let decoded = percent_encoding::percent_decode_str(encoded)
        .decode_utf8()
        .map_err(|_| invalid())?;
    let relative = decoded.strip_prefix('/').ok_or_else(invalid)?;
    if relative.is_empty() {
        return Ok("index.html".into());
    }
    if relative.contains(['\\', '\0', ':'])
        || relative
            .split('/')
            .any(|part| part.is_empty() || part.starts_with('.'))
    {
        return Err(invalid());
    }
    Ok(relative.into())
}

fn asset_mime(path: &str) -> Option<&'static str> {
    let extension = std::path::Path::new(path)
        .extension()?
        .to_str()?
        .to_ascii_lowercase();
    Some(match extension.as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" | "map" => "application/json",
        "wasm" => "application/wasm",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, PreviewAssets) {
        let dir = tempfile::tempdir().unwrap();
        let assets = PreviewAssets::new(dir.path().into()).unwrap();
        (dir, assets)
    }
    #[test]
    fn index_mime_and_encoded_names() {
        let (dir, assets) = fixture();
        for (name, mime) in [
            ("index.html", "text/html; charset=utf-8"),
            ("a b.js", "text/javascript; charset=utf-8"),
            ("main.css", "text/css; charset=utf-8"),
            ("a.wasm", "application/wasm"),
            ("font.woff2", "font/woff2"),
            ("a.png", "image/png"),
        ] {
            std::fs::write(dir.path().join(name), b"asset").unwrap();
            let path = if name == "index.html" {
                "/".into()
            } else {
                format!("/{}", name.replace(' ', "%20"))
            };
            let loaded = assets.load(&path).unwrap();
            assert_eq!(loaded.bytes, b"asset");
            assert_eq!(loaded.mime, mime);
        }
    }
    #[test]
    fn unicode_and_percent_names_decode_once() {
        let (dir, assets) = fixture();
        std::fs::write(dir.path().join("é.js"), b"unicode").unwrap();
        std::fs::write(dir.path().join("%2e%2e.js"), b"literal percent").unwrap();
        assert_eq!(assets.load("/%C3%A9.js").unwrap().bytes, b"unicode");
        assert_eq!(
            assets.load("/%252e%252e.js").unwrap().bytes,
            b"literal percent"
        );
    }
    #[cfg(unix)]
    #[test]
    fn rooted_handle_survives_rename_and_internal_link() {
        let outer = tempfile::tempdir().unwrap();
        let root = outer.path().join("root");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("index.html"), b"original").unwrap();
        std::os::unix::fs::symlink("index.html", root.join("link.html")).unwrap();
        let assets = PreviewAssets::new(root.clone()).unwrap();
        assert_eq!(assets.load("/link.html").unwrap().bytes, b"original");
        std::fs::rename(&root, outer.path().join("moved")).unwrap();
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("index.html"), b"replacement").unwrap();
        assert_eq!(assets.load("/").unwrap().bytes, b"original");
    }
    #[test]
    fn rejects_invalid_paths() {
        let (_dir, assets) = fixture();
        for path in [
            "/../index.html",
            "/%2e%2e/index.html",
            "/.env",
            "/x/.secret/a.js",
            "/a\\b.js",
            "/%00.js",
            "/%",
            "/%0",
            "/%GG",
            "/%ff",
            "//index.html",
            "/C:/index.html",
            "/%2findex.html",
        ] {
            assert_eq!(assets.load(path).err().unwrap().status, 400, "{path}");
        }
    }
    #[test]
    fn missing_directory_and_unsupported() {
        let (dir, assets) = fixture();
        std::fs::create_dir(dir.path().join("folder.html")).unwrap();
        std::fs::write(dir.path().join("config.toml"), "secret").unwrap();
        assert_eq!(assets.load("/missing.js").err().unwrap().status, 404);
        assert_eq!(assets.load("/folder.html").err().unwrap().status, 404);
        assert_eq!(assets.load("/config.toml").err().unwrap().status, 415);
    }
    #[test]
    fn bounds_size() {
        let (dir, assets) = fixture();
        let file = std::fs::File::create(dir.path().join("big.js")).unwrap();
        file.set_len(8 * 1024 * 1024).unwrap();
        assert_eq!(assets.load("/big.js").unwrap().bytes.len(), 8 * 1024 * 1024);
        file.set_len(8 * 1024 * 1024 + 1).unwrap();
        assert_eq!(assets.load("/big.js").err().unwrap().status, 413);
    }
    #[cfg(unix)]
    #[test]
    fn fifo_is_rejected_without_blocking() {
        let (dir, assets) = fixture();
        let status = std::process::Command::new("mkfifo")
            .arg(dir.path().join("pipe.js"))
            .status()
            .unwrap();
        assert!(status.success());
        assert_eq!(assets.load("/pipe.js").err().unwrap().status, 404);
    }
    #[cfg(unix)]
    #[test]
    fn outside_symlink_cannot_escape_root() {
        let (dir, assets) = fixture();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.js"), b"secret").unwrap();
        std::os::unix::fs::symlink(outside.path().join("secret.js"), dir.path().join("link.js"))
            .unwrap();
        assert!(assets.load("/link.js").is_err());
    }
}
