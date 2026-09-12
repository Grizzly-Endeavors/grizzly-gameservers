use super::*;

#[test]
fn a_zip_manifest_parses_the_fields_resolution_needs() {
    let manifest: ZipManifest = serde_json::from_str(
        r#"{"name":"Jotunn","version_number":"2.28.0","website_url":"",
            "description":"lib","dependencies":["denikson-BepInExPack_Valheim-5.4.2333"]}"#,
    )
    .unwrap();
    assert_eq!(manifest.name, "Jotunn");
    assert_eq!(manifest.version_number, "2.28.0");
    assert_eq!(manifest.dependencies.len(), 1);
}

#[test]
fn a_manifest_without_dependencies_reads_as_none_rather_than_failing() {
    let manifest: ZipManifest =
        serde_json::from_str(r#"{"name":"Sailing","version_number":"1.1.8"}"#).unwrap();
    assert!(manifest.dependencies.is_empty());
}
