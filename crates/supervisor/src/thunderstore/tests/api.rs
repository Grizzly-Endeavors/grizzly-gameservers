use std::collections::BTreeMap;
use std::io::Write as _;
use std::sync::Mutex;

use super::*;
use crate::thunderstore::cache;

#[test]
fn dependency_strings_become_releases() {
    let parsed = parse_dependencies(&[
        "denikson-BepInExPack_Valheim-5.4.2333".to_owned(),
        "ValheimModding-Jotunn-2.28.0".to_owned(),
    ]);
    assert_eq!(
        parsed.iter().map(ToString::to_string).collect::<Vec<_>>(),
        vec![
            "denikson-BepInExPack_Valheim-5.4.2333",
            "ValheimModding-Jotunn-2.28.0"
        ]
    );
}

#[test]
fn an_unparseable_dependency_is_dropped_rather_than_failing_the_launch() {
    // Upstream owns this string. Refusing the whole launch over one malformed
    // manifest would let any Thunderstore author take a running server down.
    let parsed = parse_dependencies(&[
        "totally not a dependency".to_owned(),
        "Author-Mod-1.0.0".to_owned(),
    ]);
    let [only] = parsed.as_slice() else {
        panic!("exactly one dependency survives, got {parsed:?}")
    };
    assert_eq!(only.to_string(), "Author-Mod-1.0.0");
}

// ---- the cache layering ----
//
// The asymmetry these cover is the offline story: an *exact* version is answered
// from the cache before the registry is asked at all, so a fully pinned mod list
// resolves with no network; a *latest* can only come from the registry, and falls
// back to the highest cached version with a note when that registry is down.

/// A registry that answers from a map and records what it was asked, so a test
/// can assert the cache was consulted *first* rather than merely consulted.
#[derive(Default)]
struct FakeRegistry {
    versions: BTreeMap<String, PackageVersion>,
    reachable: bool,
    asked: Mutex<Vec<String>>,
}

impl FakeRegistry {
    fn up(release: &str) -> Self {
        let parsed: PackageRelease = release.parse().unwrap();
        let mut versions = BTreeMap::new();
        versions.insert(parsed.id.to_string(), parsed.version);
        Self {
            versions,
            reachable: true,
            asked: Mutex::new(Vec::new()),
        }
    }

    fn down() -> Self {
        Self {
            reachable: false,
            ..Self::default()
        }
    }

    fn asked(&self) -> Vec<String> {
        self.asked.lock().unwrap().clone()
    }

    fn answer(
        &self,
        what: String,
        version: Option<PackageVersion>,
    ) -> Result<SourceVersion, SourceError> {
        self.asked.lock().unwrap().push(what);
        if !self.reachable {
            return Err(SourceError::Unavailable("fake outage".to_owned()));
        }
        version
            .map(|version| SourceVersion {
                version,
                dependencies: Vec::new(),
            })
            .ok_or(SourceError::NotFound)
    }
}

impl PackageSource for FakeRegistry {
    fn latest(
        &self,
        id: &PackageId,
    ) -> impl std::future::Future<Output = Result<SourceVersion, SourceError>> + Send {
        std::future::ready(self.answer(id.to_string(), self.versions.get(&id.to_string()).copied()))
    }

    fn exact(
        &self,
        release: &PackageRelease,
    ) -> impl std::future::Future<Output = Result<SourceVersion, SourceError>> + Send {
        std::future::ready(self.answer(release.to_string(), Some(release.version)))
    }
}

/// Put a real (manifest-only) package zip in the cache for `release`.
fn seed_cache(work_dir: &std::path::Path, release: &PackageRelease, dependencies: &str) {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    writer.start_file("manifest.json", options).unwrap();
    writer
        .write_all(
            format!(
                r#"{{"name":"{}","version_number":"{}","dependencies":[{dependencies}]}}"#,
                release.id.name, release.version
            )
            .as_bytes(),
        )
        .unwrap();
    let bytes = writer.finish().unwrap().into_inner();
    cache::store(work_dir, release, &bytes).unwrap();
}

#[tokio::test]
async fn a_pinned_package_resolves_from_cache_without_asking_the_registry() {
    // This is what makes a pod recreated during a Thunderstore outage come back
    // with the mods it had.
    let dir = tempfile::tempdir().unwrap();
    let release: PackageRelease = "Author-Mod-1.0.0".parse().unwrap();
    seed_cache(dir.path(), &release, r#""Lib-Core-2.0.0""#);

    let registry = FakeRegistry::down();
    let source = LayeredSource::new(&registry, dir.path());
    let found = source.exact(&release).await.unwrap();

    assert_eq!(found.version, release.version);
    assert_eq!(
        found
            .dependencies
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        vec!["Lib-Core-2.0.0"],
        "the cached zip's own manifest is where the dependencies come from"
    );
    assert!(
        registry.asked().is_empty(),
        "the registry must not be asked at all when the cache answers"
    );
    assert!(
        source.take_notes().is_empty(),
        "a cache hit is not a degradation"
    );
}

#[tokio::test]
async fn a_pinned_package_that_is_not_cached_falls_through_to_the_registry() {
    let dir = tempfile::tempdir().unwrap();
    let release: PackageRelease = "Author-Mod-1.0.0".parse().unwrap();
    let registry = FakeRegistry::up("Author-Mod-1.0.0");
    let source = LayeredSource::new(&registry, dir.path());

    assert_eq!(
        source.exact(&release).await.unwrap().version,
        release.version
    );
    assert_eq!(registry.asked(), vec!["Author-Mod-1.0.0"]);
}

#[tokio::test]
async fn an_unpinned_package_prefers_the_registry_even_when_something_is_cached() {
    // "Latest" is a question only the registry can answer, so a cached copy must
    // not stand in while it is reachable — that would pin an unpinned entry.
    let dir = tempfile::tempdir().unwrap();
    seed_cache(dir.path(), &"Author-Mod-1.0.0".parse().unwrap(), "");
    let registry = FakeRegistry::up("Author-Mod-2.0.0");
    let source = LayeredSource::new(&registry, dir.path());

    let found = source.latest(&"Author-Mod".parse().unwrap()).await.unwrap();
    assert_eq!(found.version.to_string(), "2.0.0");
    assert!(source.take_notes().is_empty());
}

#[tokio::test]
async fn an_unpinned_package_falls_back_to_the_highest_cached_version_and_says_so() {
    let dir = tempfile::tempdir().unwrap();
    seed_cache(dir.path(), &"Author-Mod-1.0.0".parse().unwrap(), "");
    seed_cache(dir.path(), &"Author-Mod-1.4.0".parse().unwrap(), "");
    seed_cache(dir.path(), &"Author-Mod-1.2.0".parse().unwrap(), "");

    let registry = FakeRegistry::down();
    let source = LayeredSource::new(&registry, dir.path());
    let found = source.latest(&"Author-Mod".parse().unwrap()).await.unwrap();

    assert_eq!(
        found.version.to_string(),
        "1.4.0",
        "highest, not newest-written"
    );
    let notes = source.take_notes();
    let [note] = notes.as_slice() else {
        panic!("the degradation must be reported once, got {notes:?}")
    };
    assert!(note.contains("Author-Mod"), "{note}");
    assert!(note.contains("1.4.0"), "{note}");
}

#[tokio::test]
async fn an_unreachable_registry_with_an_empty_cache_is_a_failure_not_a_guess() {
    let dir = tempfile::tempdir().unwrap();
    let registry = FakeRegistry::down();
    let source = LayeredSource::new(&registry, dir.path());

    assert!(matches!(
        source.latest(&"Author-Mod".parse().unwrap()).await,
        Err(SourceError::Unavailable(_))
    ));
}

#[tokio::test]
async fn a_package_the_registry_denies_is_never_served_from_cache() {
    // NotFound is a mistake in the mod list, not an outage. Falling back here
    // would install something nobody can look up.
    let dir = tempfile::tempdir().unwrap();
    seed_cache(dir.path(), &"Author-Mod-1.0.0".parse().unwrap(), "");
    let registry = FakeRegistry::up("Someone-Else-1.0.0");
    let source = LayeredSource::new(&registry, dir.path());

    assert_eq!(
        source.latest(&"Author-Mod".parse().unwrap()).await,
        Err(SourceError::NotFound)
    );
}
