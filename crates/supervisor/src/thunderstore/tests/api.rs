use super::*;

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
