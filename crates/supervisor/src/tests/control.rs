use super::*;

#[tokio::test]
async fn fs_error_maps_each_variant_to_the_right_status() {
    let cases = [
        (FsError::OutsideRoot, StatusCode::FORBIDDEN),
        (FsError::NotFound, StatusCode::NOT_FOUND),
        (FsError::NoBackup, StatusCode::NOT_FOUND),
        (FsError::NotAFile, StatusCode::BAD_REQUEST),
        (FsError::NotADirectory, StatusCode::BAD_REQUEST),
        (FsError::NotText, StatusCode::BAD_REQUEST),
        (FsError::TooLarge, StatusCode::BAD_REQUEST),
        (
            FsError::Io("disk on fire".to_owned()),
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
    ];

    for (err, expected) in cases {
        let response = fs_error("read", "/world/level.dat", &err);
        assert_eq!(
            response.status(),
            expected,
            "{err:?} should map to {expected}"
        );
    }
}
