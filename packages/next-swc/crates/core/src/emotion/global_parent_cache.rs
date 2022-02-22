use std::path::{Path, PathBuf};

use fxhash::FxHashMap;
use once_cell::sync::Lazy;
use serde::Deserialize;
use serde_json::from_reader;
use swc_common::sync::RwLock;

pub(crate) static GLOBAL_PARENT_CACHE: Lazy<GlobalParentCache> = Lazy::new(GlobalParentCache::new);

#[derive(Deserialize, Debug, Clone)]
struct PackageJson {
    name: String,
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub(crate) struct RootPathInfo {
    pub(crate) package_name: String,
    pub(crate) root_path: PathBuf,
}

impl RootPathInfo {
    pub(crate) fn new(package_name: String, root_path: PathBuf) -> Self {
        Self {
            package_name,
            root_path,
        }
    }
}

pub(crate) struct GlobalParentCache {
    cache: RwLock<FxHashMap<PathBuf, RootPathInfo>>,
}

impl GlobalParentCache {
    fn new() -> Self {
        Self {
            cache: RwLock::new(FxHashMap::default()),
        }
    }
}

impl GlobalParentCache {
    pub(crate) fn get(&self, p: &Path) -> Option<RootPathInfo> {
        let guard = self.cache.read();
        guard.get(p).cloned()
    }

    pub(crate) fn insert(&self, p: PathBuf, parent: PathBuf) -> RootPathInfo {
        let mut write_lock = self.cache.borrow_mut();
        // Safe to unwrap, because `existed` is true
        let file = std::fs::File::open(parent.join("package.json")).unwrap();
        let package_json: PackageJson = from_reader(file).unwrap();
        let info = RootPathInfo {
            package_name: package_json.name,
            root_path: parent,
        };
        write_lock.insert(p, info.clone());
        info
    }
}
