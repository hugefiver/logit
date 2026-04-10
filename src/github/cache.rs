use std::path::PathBuf;

pub struct DiskCache {
    cache_dir: PathBuf,
}

impl DiskCache {
    pub fn new() -> anyhow::Result<Self> {
        let cache_dir = dirs_or_fallback();
        std::fs::create_dir_all(&cache_dir)?;
        Ok(Self { cache_dir })
    }

    #[cfg(test)]
    pub fn with_dir<P: AsRef<std::path::Path>>(cache_dir: P) -> anyhow::Result<Self> {
        let cache_dir = cache_dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&cache_dir)?;
        Ok(Self { cache_dir })
    }

    pub fn get<T: serde::de::DeserializeOwned>(&self, key: &str) -> Option<T> {
        let path = self.cache_dir.join(format!("{key}.json"));
        let data = std::fs::read_to_string(&path).ok()?;
        serde_json::from_str(&data).ok()
    }

    pub fn set<T: serde::Serialize>(&self, key: &str, value: &T) -> anyhow::Result<()> {
        let path = self.cache_dir.join(format!("{key}.json"));
        let data = serde_json::to_string(value)?;
        std::fs::write(path, data)?;
        Ok(())
    }
}

fn dirs_or_fallback() -> PathBuf {
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        PathBuf::from(local)
            .join("logit")
            .join("cache")
            .join("github")
    } else if let Ok(home) = std::env::var("HOME") {
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
    use super::DiskCache;

    #[test]
    fn cache_set_get_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = DiskCache::with_dir(tmp.path()).unwrap();

        cache.set("user_octocat", &vec![1u64, 2, 3]).unwrap();
        let restored: Option<Vec<u64>> = cache.get("user_octocat");

        assert_eq!(restored, Some(vec![1, 2, 3]));
    }

    #[test]
    fn cache_returns_none_for_missing_key() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = DiskCache::with_dir(tmp.path()).unwrap();

        let result: Option<String> = cache.get("nonexistent");
        assert!(result.is_none());
    }

    #[test]
    fn cache_overwrites_existing_key() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = DiskCache::with_dir(tmp.path()).unwrap();

        cache.set("key", &"first").unwrap();
        cache.set("key", &"second").unwrap();
        let restored: Option<String> = cache.get("key");
        assert_eq!(restored, Some("second".to_string()));
    }
}
