use super::*;

#[tokio::test]
async fn no_config_yields_a_disabled_client() {
    let client = ValkeyClient::connect(None).await;
    assert!(
        !client.is_enabled(),
        "an unconfigured client must report itself disabled"
    );
}

#[tokio::test]
async fn disabled_client_errors_rather_than_panics() {
    // Every operation on a disabled client returns an error (which run_when maps to
    // a friendly "can't schedule" reply), never a panic.
    let client = ValkeyClient::connect(None).await;
    assert!(
        client
            .rpush("gameservers:wait:mc:empty", "{}")
            .await
            .is_err()
    );
    assert!(
        client
            .expire("gameservers:wait:mc:empty", 60)
            .await
            .is_err()
    );
    assert!(client.drain("gameservers:wait:mc:empty").await.is_err());
    assert!(client.is_empty("gameservers:wait:mc:empty").await.is_err());
    assert!(client.scan_keys("gameservers:wait:*").await.is_err());
}
