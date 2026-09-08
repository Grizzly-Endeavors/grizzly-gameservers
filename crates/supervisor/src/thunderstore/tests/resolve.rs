use std::collections::BTreeMap;

use super::*;

/// A source backed by a map, so every resolution rule is exercised without a
/// network or a cache.
#[derive(Default)]
struct FakeSource {
    /// Every version this registry knows, keyed by release string.
    versions: BTreeMap<String, Vec<String>>,
    /// Packages that are unreachable rather than absent.
    unreachable: Vec<String>,
}

impl FakeSource {
    fn with(mut self, release: &str, dependencies: &[&str]) -> Self {
        self.versions.insert(
            release.to_owned(),
            dependencies.iter().map(|d| (*d).to_owned()).collect(),
        );
        self
    }

    fn unreachable(mut self, id: &str) -> Self {
        self.unreachable.push(id.to_owned());
        self
    }

    fn lookup(&self, release: &PackageRelease) -> Result<SourceVersion, SourceError> {
        if self.unreachable.contains(&release.id.to_string()) {
            return Err(SourceError::Unavailable("fake outage".to_owned()));
        }
        let dependencies = self
            .versions
            .get(&release.to_string())
            .ok_or(SourceError::NotFound)?;
        Ok(SourceVersion {
            version: release.version,
            dependencies: dependencies
                .iter()
                .map(|raw| raw.parse().expect("fixture dependency parses"))
                .collect(),
        })
    }
}

impl PackageSource for FakeSource {
    async fn latest(&self, id: &PackageId) -> Result<SourceVersion, SourceError> {
        if self.unreachable.contains(&id.to_string()) {
            return Err(SourceError::Unavailable("fake outage".to_owned()));
        }
        let prefix = format!("{id}-");
        let highest = self
            .versions
            .keys()
            .filter_map(|release| release.strip_prefix(&prefix))
            .filter_map(|version| version.parse::<PackageVersion>().ok())
            .max()
            .ok_or(SourceError::NotFound)?;
        self.lookup(&PackageRelease {
            id: id.clone(),
            version: highest,
        })
    }

    async fn exact(&self, release: &PackageRelease) -> Result<SourceVersion, SourceError> {
        self.lookup(release)
    }
}

fn wanted(line: &str, line_number: usize) -> Wanted {
    match line.parse::<PackageRelease>() {
        Ok(release) => Wanted {
            id: release.id,
            pin: Some(release.version),
            line_number,
        },
        Err(_) => Wanted {
            id: line.parse().expect("fixture package parses"),
            pin: None,
            line_number,
        },
    }
}

fn installed(resolution: &Resolution) -> Vec<String> {
    resolution
        .packages
        .iter()
        .map(|package| package.release.to_string())
        .collect()
}

async fn resolve_lines(source: &FakeSource, lines: &[&str], provided: &[&str]) -> Resolution {
    let wants: Vec<Wanted> = lines
        .iter()
        .enumerate()
        .map(|(index, line)| wanted(line, index + 1))
        .collect();
    let provided: Vec<PackageId> = provided
        .iter()
        .map(|id| id.parse().expect("fixture id parses"))
        .collect();
    resolve(source, &wants, &provided)
        .await
        .expect("fixture resolves")
}

#[tokio::test]
async fn an_unpinned_entry_takes_the_newest_version() {
    let source = FakeSource::default()
        .with("Author-Mod-1.0.0", &[])
        .with("Author-Mod-1.2.0", &[]);
    let resolution = resolve_lines(&source, &["Author-Mod"], &[]).await;
    assert_eq!(installed(&resolution), vec!["Author-Mod-1.2.0"]);
    assert!(!resolution.packages.first().unwrap().pinned);
}

#[tokio::test]
async fn a_pinned_entry_takes_exactly_what_it_names() {
    let source = FakeSource::default()
        .with("Author-Mod-1.0.0", &[])
        .with("Author-Mod-1.2.0", &[]);
    let resolution = resolve_lines(&source, &["Author-Mod-1.0.0"], &[]).await;
    assert_eq!(installed(&resolution), vec!["Author-Mod-1.0.0"]);
    assert!(resolution.packages.first().unwrap().pinned);
}

#[tokio::test]
async fn dependencies_are_installed_transitively() {
    let source = FakeSource::default()
        .with("Author-Mod-1.0.0", &["Lib-Core-2.0.0"])
        .with("Lib-Core-2.0.0", &["Lib-Deep-3.0.0"])
        .with("Lib-Deep-3.0.0", &[]);
    let resolution = resolve_lines(&source, &["Author-Mod-1.0.0"], &[]).await;
    assert_eq!(
        installed(&resolution),
        vec!["Author-Mod-1.0.0", "Lib-Core-2.0.0", "Lib-Deep-3.0.0"]
    );
}

#[tokio::test]
async fn the_highest_dependency_version_wins() {
    let source = FakeSource::default()
        .with("Author-One-1.0.0", &["Lib-Core-2.0.0"])
        .with("Author-Two-1.0.0", &["Lib-Core-2.5.0"])
        .with("Lib-Core-2.0.0", &[])
        .with("Lib-Core-2.5.0", &[]);
    for order in [
        ["Author-One-1.0.0", "Author-Two-1.0.0"],
        ["Author-Two-1.0.0", "Author-One-1.0.0"],
    ] {
        let resolution = resolve_lines(&source, &order, &[]).await;
        assert!(
            installed(&resolution).contains(&"Lib-Core-2.5.0".to_owned()),
            "the highest floor must win regardless of list order: {order:?}"
        );
    }
}

#[tokio::test]
async fn a_pin_outranks_a_dependency_asking_for_more_and_says_so() {
    // Thunderstore manifests declare what a package was built against, not what
    // it requires, so a newer "requirement" is a preference. The friend's pin
    // has to survive it.
    let source = FakeSource::default()
        .with("Author-Mod-1.0.0", &["Lib-Core-2.5.0"])
        .with("Lib-Core-2.0.0", &[])
        .with("Lib-Core-2.5.0", &[]);
    let resolution = resolve_lines(&source, &["Author-Mod-1.0.0", "Lib-Core-2.0.0"], &[]).await;
    assert!(installed(&resolution).contains(&"Lib-Core-2.0.0".to_owned()));
    assert_eq!(resolution.warnings.len(), 1);
    let warning = resolution.warnings.first().unwrap().to_string();
    assert!(warning.contains("Lib-Core-2.0.0"), "got: {warning}");
    assert!(warning.contains("2.5.0"), "got: {warning}");
}

#[tokio::test]
async fn a_pin_reached_first_as_a_dependency_still_holds() {
    // Order trap: resolving the dependency before the mod list's own entry
    // would record the dependency's version and quietly discard the pin.
    let source = FakeSource::default()
        .with("Author-Mod-1.0.0", &["Lib-Core-2.5.0"])
        .with("Lib-Core-2.0.0", &[])
        .with("Lib-Core-2.5.0", &[]);
    let resolution = resolve_lines(&source, &["Author-Mod-1.0.0", "Lib-Core-2.0.0"], &[]).await;
    let core = resolution
        .packages
        .iter()
        .find(|package| package.release.id.name == "Core")
        .unwrap();
    assert_eq!(core.release.version.to_string(), "2.0.0");
    assert!(core.pinned);
}

#[tokio::test]
async fn a_provided_package_is_skipped_whole() {
    // The image ships the loader pack as a unit; installing pieces of a second
    // copy over it is how a working loader stops loading.
    let source = FakeSource::default()
        .with(
            "Author-Mod-1.0.0",
            &["denikson-BepInExPack_Valheim-5.4.2333"],
        )
        .with(
            "denikson-BepInExPack_Valheim-5.4.2333",
            &["Lib-Never-1.0.0"],
        );
    let resolution = resolve_lines(
        &source,
        &["Author-Mod-1.0.0"],
        &["denikson-BepInExPack_Valheim"],
    )
    .await;
    assert_eq!(installed(&resolution), vec!["Author-Mod-1.0.0"]);
    assert_eq!(
        resolution.provided,
        vec!["denikson-BepInExPack_Valheim".parse::<PackageId>().unwrap()]
    );
}

#[tokio::test]
async fn a_provided_package_listed_by_hand_is_skipped_too() {
    let source = FakeSource::default().with("denikson-BepInExPack_Valheim-5.4.2333", &[]);
    let resolution = resolve_lines(
        &source,
        &["denikson-BepInExPack_Valheim-5.4.2333"],
        &["denikson-BepInExPack_Valheim"],
    )
    .await;
    assert!(resolution.packages.is_empty());
    assert_eq!(resolution.provided.len(), 1);
}

#[tokio::test]
async fn a_dependency_cycle_settles_instead_of_spinning() {
    let source = FakeSource::default()
        .with("Author-One-1.0.0", &["Author-Two-1.0.0"])
        .with("Author-Two-1.0.0", &["Author-One-1.0.0"]);
    let resolution = resolve_lines(&source, &["Author-One-1.0.0"], &[]).await;
    assert_eq!(
        installed(&resolution),
        vec!["Author-One-1.0.0", "Author-Two-1.0.0"]
    );
}

#[tokio::test]
async fn an_unknown_package_names_the_line_that_asked_for_it() {
    let source = FakeSource::default();
    let wants = vec![wanted("Nobody-DoesNotExist", 7)];
    let err = resolve(&source, &wants, &[]).await.unwrap_err();
    let message = err.to_string();
    assert!(message.contains("Nobody-DoesNotExist"), "got: {message}");
    assert!(message.contains("line 7"), "got: {message}");
}

#[tokio::test]
async fn a_missing_dependency_names_the_package_that_needs_it() {
    let source = FakeSource::default().with("Author-Mod-1.0.0", &["Lib-Gone-1.0.0"]);
    let wants = vec![wanted("Author-Mod-1.0.0", 1)];
    let err = resolve(&source, &wants, &[]).await.unwrap_err();
    let message = err.to_string();
    assert!(message.contains("Lib-Gone-1.0.0"), "got: {message}");
    assert!(message.contains("Author-Mod-1.0.0"), "got: {message}");
}

#[tokio::test]
async fn an_unreachable_registry_reads_differently_from_a_missing_package() {
    // The same launch failure, but a completely different thing to tell a
    // friend: one is their typo, the other is Thunderstore being down.
    let source = FakeSource::default().unreachable("Author-Mod");
    let wants = vec![wanted("Author-Mod", 1)];
    let err = resolve(&source, &wants, &[]).await.unwrap_err();
    assert!(
        err.to_string().contains("could not be reached"),
        "got: {err}"
    );
}

#[tokio::test]
async fn the_package_cap_stops_a_runaway_list() {
    let mut source = FakeSource::default();
    for index in 0..=MAX_PACKAGES {
        source = source.with(&format!("Author-Mod{index}-1.0.0"), &[]);
    }
    let wants: Vec<Wanted> = (0..=MAX_PACKAGES)
        .map(|index| wanted(&format!("Author-Mod{index}-1.0.0"), index + 1))
        .collect();
    let err = resolve(&source, &wants, &[]).await.unwrap_err();
    assert!(matches!(err, ResolveError::TooManyPackages { .. }), "{err}");
}

#[tokio::test]
async fn the_depth_cap_stops_a_runaway_chain() {
    let mut source = FakeSource::default();
    let depth = MAX_DEPTH + 3;
    for index in 0..depth {
        source = source.with(
            &format!("Author-Mod{index}-1.0.0"),
            &[&format!("Author-Mod{}-1.0.0", index + 1)],
        );
    }
    source = source.with(&format!("Author-Mod{depth}-1.0.0"), &[]);
    let wants = vec![wanted("Author-Mod0-1.0.0", 1)];
    let err = resolve(&source, &wants, &[]).await.unwrap_err();
    assert!(matches!(err, ResolveError::TooDeep { .. }), "{err}");
}

#[tokio::test]
async fn the_installed_set_is_ordered_so_the_resolved_file_is_stable() {
    let source = FakeSource::default()
        .with("Zed-Last-1.0.0", &[])
        .with("Alpha-First-1.0.0", &[]);
    let resolution = resolve_lines(&source, &["Zed-Last-1.0.0", "Alpha-First-1.0.0"], &[]).await;
    assert_eq!(
        installed(&resolution),
        vec!["Alpha-First-1.0.0", "Zed-Last-1.0.0"]
    );
}
