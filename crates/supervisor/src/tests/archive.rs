use super::*;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).expect("read back extracted file")
}

#[tokio::test]
async fn create_then_extract_round_trips_a_tree() {
    let src = tempfile::tempdir().expect("src tempdir");
    std::fs::write(src.path().join("server.properties"), "difficulty=hard\n").unwrap();
    std::fs::create_dir(src.path().join("world")).unwrap();
    std::fs::write(src.path().join("world").join("level.dat"), "seed=42").unwrap();

    // Stream the archive out fully into memory (the test tree is tiny).
    let mut creator = spawn_create(src.path()).expect("spawn create");
    let mut stdout = creator.stdout.take().expect("create stdout");
    let mut tarball = Vec::new();
    stdout
        .read_to_end(&mut tarball)
        .await
        .expect("read archive");
    assert!(
        creator.wait().await.expect("wait create").success(),
        "tar create should exit cleanly"
    );
    assert!(!tarball.is_empty(), "archive should carry bytes");

    // Feed it back into a fresh root.
    let dst = tempfile::tempdir().expect("dst tempdir");
    let mut extractor = spawn_extract(dst.path()).expect("spawn extract");
    let mut stdin = extractor.stdin.take().expect("extract stdin");
    stdin.write_all(&tarball).await.expect("write archive");
    stdin.flush().await.expect("flush");
    drop(stdin);
    assert!(
        extractor.wait().await.expect("wait extract").success(),
        "tar extract should exit cleanly"
    );

    assert_eq!(
        read(&dst.path().join("server.properties")),
        "difficulty=hard\n"
    );
    assert_eq!(read(&dst.path().join("world").join("level.dat")), "seed=42");
}

#[test]
fn purge_removes_children_but_keeps_reserved_entries() {
    let root = tempfile::tempdir().expect("root tempdir");
    std::fs::write(root.path().join("server.properties"), "x").unwrap();
    std::fs::create_dir(root.path().join("world")).unwrap();
    std::fs::write(root.path().join("world").join("level.dat"), "y").unwrap();
    std::fs::create_dir(root.path().join("lost+found")).unwrap();

    purge(root.path()).expect("purge");

    let remaining: Vec<_> = std::fs::read_dir(root.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        remaining,
        vec!["lost+found".to_owned()],
        "purge should clear the world but leave lost+found"
    );
}

#[test]
fn purge_on_empty_root_is_a_noop() {
    let root = tempfile::tempdir().expect("root tempdir");
    purge(root.path()).expect("purge empty");
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
}
