use std::{ffi::OsString, path::PathBuf};

use anyhow::Context;

pub struct DiskCache {
    cache_dir: PathBuf,
}

impl DiskCache {
    pub fn new() -> anyhow::Result<Self> {
        let cache_dir = dirs_or_fallback();
        std::fs::create_dir_all(&cache_dir).with_context(|| {
            format!(
                "failed to initialize GitHub cache at {}",
                cache_dir.display()
            )
        })?;
        Ok(Self { cache_dir })
    }

    #[cfg(test)]
    pub fn with_dir<P: AsRef<std::path::Path>>(cache_dir: P) -> anyhow::Result<Self> {
        let cache_dir = cache_dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&cache_dir).with_context(|| {
            format!(
                "failed to initialize GitHub cache at {}",
                cache_dir.display()
            )
        })?;
        Ok(Self { cache_dir })
    }

    pub fn get<T: serde::de::DeserializeOwned>(&self, key: &str) -> anyhow::Result<Option<T>> {
        let path = self.cache_dir.join(format!("{key}.json"));
        let data = match std::fs::read_to_string(&path) {
            Ok(data) => data,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("failed to read cache entry '{key}' at {}", path.display())
                });
            }
        };

        serde_json::from_str(&data)
            .map(Some)
            .with_context(|| format!("failed to parse cache entry '{key}' at {}", path.display()))
    }

    pub fn set<T: serde::Serialize>(&self, key: &str, value: &T) -> anyhow::Result<()> {
        let path = self.cache_dir.join(format!("{key}.json"));
        let data = serde_json::to_string(value).with_context(|| {
            format!(
                "failed to serialize cache entry '{key}' for {}",
                path.display()
            )
        })?;
        std::fs::write(&path, data).with_context(|| {
            format!("failed to write cache entry '{key}' at {}", path.display())
        })?;
        Ok(())
    }
}

fn dirs_or_fallback() -> PathBuf {
    dirs_or_fallback_with(|name| std::env::var_os(name))
}

fn dirs_or_fallback_with(get_env: impl Fn(&str) -> Option<OsString>) -> PathBuf {
    if let Some(cache_dir) = get_env("LOGIT_GITHUB_CACHE_DIR").filter(|path| !path.is_empty()) {
        return PathBuf::from(cache_dir);
    }

    if let Some(local) = get_env("LOCALAPPDATA") {
        PathBuf::from(local)
            .join("logit")
            .join("cache")
            .join("github")
    } else if let Some(home) = get_env("HOME") {
        PathBuf::from(home)
            .join(".cache")
            .join("logit")
            .join("github")
    } else {
        PathBuf::from(".logit-cache").join("github")
    }
}

#[cfg(test)]
mod tests {
    use super::{DiskCache, dirs_or_fallback_with};
    use std::{ffi::OsString, fs, path::PathBuf};

    #[test]
    fn github_cache_dir_selection_prefers_exact_override_and_preserves_fallback() {
        let expected = PathBuf::from("C:/runner-temp/logit-github-cache");
        let actual = dirs_or_fallback_with(|name| {
            (name == "LOGIT_GITHUB_CACHE_DIR").then(|| expected.clone().into_os_string())
        });

        assert_eq!(actual, expected);

        let actual = dirs_or_fallback_with(|name| match name {
            "LOCALAPPDATA" => Some(PathBuf::from("C:/Local").into_os_string()),
            _ => None,
        });

        assert_eq!(actual, PathBuf::from("C:/Local/logit/cache/github"));

        let actual = dirs_or_fallback_with(|name| match name {
            "LOGIT_GITHUB_CACHE_DIR" => Some(OsString::new()),
            "LOCALAPPDATA" => Some(PathBuf::from("C:/Local").into_os_string()),
            _ => None,
        });

        assert_eq!(actual, PathBuf::from("C:/Local/logit/cache/github"));
    }

    #[test]
    fn cache_set_get_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = DiskCache::with_dir(tmp.path()).unwrap();

        cache.set("user_octocat", &vec![1u64, 2, 3]).unwrap();
        let restored: Option<Vec<u64>> = cache.get("user_octocat").unwrap();

        assert_eq!(restored, Some(vec![1, 2, 3]));
    }

    #[test]
    fn cache_returns_none_for_missing_key() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = DiskCache::with_dir(tmp.path()).unwrap();

        let result: Option<String> = cache.get("nonexistent").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn cache_overwrites_existing_key() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = DiskCache::with_dir(tmp.path()).unwrap();

        cache.set("key", &"first").unwrap();
        cache.set("key", &"second").unwrap();
        let restored: Option<String> = cache.get("key").unwrap();
        assert_eq!(restored, Some("second".to_string()));
    }

    #[test]
    fn malformed_cache_is_distinct_from_miss() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = DiskCache::with_dir(tmp.path()).unwrap();
        let malformed_path = tmp.path().join("malformed.json");
        fs::write(&malformed_path, "not valid JSON").unwrap();

        let malformed = cache.get::<String>("malformed").unwrap_err();
        assert!(malformed.to_string().contains("malformed"));
        assert!(
            malformed
                .to_string()
                .contains(&malformed_path.display().to_string())
        );

        let missing: Option<String> = cache.get("missing").unwrap();
        assert_eq!(missing, None);
    }
}
