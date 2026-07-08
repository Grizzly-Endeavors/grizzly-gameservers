//! The S3 shell over [`rusty_s3`]: it only *signs* requests (`SigV4` presigned
//! URLs) which the bot's own `reqwest` client then executes, so no second HTTP or
//! TLS stack enters the tree. Backups stream up as a multipart upload and down as
//! a body stream, so a multi-gigabyte world never fully buffers in the bot's
//! read-only pod (only one ~16 MiB part at a time).
//!
//! The store points at the self-hosted versitygw `s3-bulk` endpoint with
//! path-style addressing; credentials arrive via ESO env, never a runtime fetch.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use rusty_s3::actions::{CreateMultipartUpload, ListObjectsV2};
use rusty_s3::{Bucket, Credentials, S3Action, UrlStyle};
use url::Url;

use super::manifest::BackupManifest;
use crate::config::S3Config;

/// Size each multipart part is flushed at. Above S3's 5 MiB non-final-part floor,
/// and the only buffer the bot holds in memory during an upload.
const PART_SIZE: usize = 16 * 1024 * 1024;

/// S3's hard cap on the number of parts in one multipart upload. Enforced
/// preemptively so an oversized archive fails with a clear message instead of
/// being signed and PUT part-by-part until the server rejects it opaquely.
const S3_MAX_PARTS: usize = 10_000;

/// Validity window for each presigned URL. Generous relative to a single part
/// upload or list over the LAN, so a slow world never outruns its own signature.
const SIGN_EXPIRY: Duration = Duration::from_hours(1);

/// A configured handle to the backups bucket: the signer plus the shared reqwest
/// client that runs the signed requests.
pub(crate) struct S3Store {
    bucket: Bucket,
    credentials: Credentials,
    http: reqwest::Client,
}

impl S3Store {
    /// Build the store from config. `http` should be a long-timeout client — the
    /// same one used to stream the archive to/from the supervisor.
    ///
    /// # Errors
    ///
    /// Returns an error if the endpoint URL is invalid or the bucket can't be
    /// constructed.
    pub(crate) fn new(config: &S3Config, http: reqwest::Client) -> Result<Self> {
        let endpoint = Url::parse(&config.endpoint)
            .with_context(|| format!("invalid s3 endpoint {}", config.endpoint))?;
        let bucket = Bucket::new(
            endpoint,
            UrlStyle::Path,
            config.bucket.clone(),
            config.region.clone(),
        )
        .context("failed to construct s3 bucket handle")?;
        let credentials = Credentials::new(config.access_key.clone(), config.secret_key.clone());
        Ok(Self {
            bucket,
            credentials,
            http,
        })
    }

    /// Stream the body of `source` up to `key` as a multipart upload, returning
    /// the number of bytes stored. Aborts the multipart upload on any failure so a
    /// broken transfer doesn't leave orphaned parts accruing storage.
    ///
    /// # Errors
    ///
    /// Returns an error if the source can't be read or any S3 step fails.
    pub(crate) async fn upload_stream(&self, key: &str, source: reqwest::Response) -> Result<u64> {
        let upload_id = self.create_multipart(key).await?;
        match self.stream_parts(key, &upload_id, source).await {
            Ok(total) => Ok(total),
            Err(err) => {
                self.abort_multipart(key, &upload_id).await;
                Err(err)
            }
        }
    }

    async fn create_multipart(&self, key: &str) -> Result<String> {
        let action = self
            .bucket
            .create_multipart_upload(Some(&self.credentials), key);
        let url = action.sign(SIGN_EXPIRY);
        let response = self
            .http
            .post(url)
            .send()
            .await
            .with_context(|| format!("failed to start multipart upload for {key}"))?;
        let body = success_text(response).await?;
        let parsed = CreateMultipartUpload::parse_response(&body)
            .context("failed to parse multipart-create response")?;
        Ok(parsed.upload_id().to_owned())
    }

    async fn stream_parts(
        &self,
        key: &str,
        upload_id: &str,
        mut source: reqwest::Response,
    ) -> Result<u64> {
        let mut etags: Vec<String> = Vec::new();
        let mut buf: Vec<u8> = Vec::with_capacity(PART_SIZE);
        let mut total: u64 = 0;
        while let Some(chunk) = source
            .chunk()
            .await
            .context("failed reading the archive stream")?
        {
            total = total.saturating_add(chunk.len() as u64);
            buf.extend_from_slice(&chunk);
            if buf.len() >= PART_SIZE {
                let part = std::mem::take(&mut buf);
                etags.push(self.upload_part(key, upload_id, etags.len(), part).await?);
                buf.reserve(PART_SIZE);
            }
        }
        // Flush the tail; also covers a sub-part-size archive, which still needs
        // at least one part before the upload can be completed.
        if !buf.is_empty() || etags.is_empty() {
            etags.push(self.upload_part(key, upload_id, etags.len(), buf).await?);
        }
        self.complete_multipart(key, upload_id, &etags).await?;
        Ok(total)
    }

    async fn upload_part(
        &self,
        key: &str,
        upload_id: &str,
        already_uploaded: usize,
        data: Vec<u8>,
    ) -> Result<String> {
        // Part numbers are 1-based; S3 caps a multipart upload at S3_MAX_PARTS,
        // which fits comfortably in the u16 that `upload_part` wants.
        let part_index = already_uploaded + 1;
        anyhow::ensure!(
            part_index <= S3_MAX_PARTS,
            "archive exceeds the {S3_MAX_PARTS}-part multipart limit"
        );
        let part_number =
            u16::try_from(part_index).context("multipart part number overflowed u16")?;
        let action = self
            .bucket
            .upload_part(Some(&self.credentials), key, part_number, upload_id);
        let url = action.sign(SIGN_EXPIRY);
        let response = self
            .http
            .put(url)
            .body(data)
            .send()
            .await
            .with_context(|| format!("failed to upload part {part_number} for {key}"))?;
        let response = success(response).await?;
        response
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
            .with_context(|| format!("upload of part {part_number} returned no ETag"))
    }

    async fn complete_multipart(&self, key: &str, upload_id: &str, etags: &[String]) -> Result<()> {
        let action = self.bucket.complete_multipart_upload(
            Some(&self.credentials),
            key,
            upload_id,
            etags.iter().map(String::as_str),
        );
        let url = action.sign(SIGN_EXPIRY);
        let body = action.body();
        let response = self
            .http
            .post(url)
            .body(body)
            .send()
            .await
            .with_context(|| format!("failed to complete multipart upload for {key}"))?;
        success(response).await.map(drop)
    }

    async fn abort_multipart(&self, key: &str, upload_id: &str) {
        let action = self
            .bucket
            .abort_multipart_upload(Some(&self.credentials), key, upload_id);
        let url = action.sign(SIGN_EXPIRY);
        match self.http.delete(url).send().await {
            Ok(response) if response.status().is_success() => {}
            Ok(response) => {
                tracing::warn!(status = %response.status(), key, "failed to abort orphaned multipart upload");
            }
            Err(err) => {
                tracing::warn!(error = ?err, key, "failed to abort orphaned multipart upload");
            }
        }
    }

    /// Open a streaming GET of `key`, returning the checked response whose body
    /// the caller pipes into the supervisor's restore route.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or the object is missing.
    pub(crate) async fn download_stream(&self, key: &str) -> Result<reqwest::Response> {
        let action = self.bucket.get_object(Some(&self.credentials), key);
        let url = action.sign(SIGN_EXPIRY);
        let response = self
            .http
            .get(url)
            .send()
            .await
            .with_context(|| format!("failed to download {key}"))?;
        success(response).await
    }

    /// Upload the manifest sidecar for an artifact.
    ///
    /// # Errors
    ///
    /// Returns an error if the manifest can't be encoded or the upload fails.
    pub(crate) async fn put_manifest(&self, key: &str, manifest: &BackupManifest) -> Result<()> {
        let body = serde_json::to_vec(manifest).context("failed to encode backup manifest")?;
        let action = self.bucket.put_object(Some(&self.credentials), key);
        let url = action.sign(SIGN_EXPIRY);
        let response = self
            .http
            .put(url)
            .body(body)
            .send()
            .await
            .with_context(|| format!("failed to upload manifest {key}"))?;
        success(response).await.map(drop)
    }

    /// Fetch and parse an artifact's manifest sidecar.
    ///
    /// # Errors
    ///
    /// Returns an error if the fetch fails or the body isn't a valid manifest.
    pub(crate) async fn get_manifest(&self, key: &str) -> Result<BackupManifest> {
        let response = self.download_stream(key).await?;
        let bytes = response
            .bytes()
            .await
            .with_context(|| format!("failed to read manifest body {key}"))?;
        serde_json::from_slice(&bytes).with_context(|| format!("failed to parse manifest {key}"))
    }

    /// The `.tar.zst` object keys under `prefix`, newest-or-oldest agnostic (the
    /// caller sorts). Non-tarball keys (manifests) are filtered out.
    ///
    /// # Errors
    ///
    /// Returns an error if the listing fails or can't be parsed.
    pub(crate) async fn list_tarballs(&self, prefix: &str) -> Result<Vec<String>> {
        let mut action = self.bucket.list_objects_v2(Some(&self.credentials));
        action.with_prefix(prefix);
        let url = action.sign(SIGN_EXPIRY);
        let response = self
            .http
            .get(url)
            .send()
            .await
            .with_context(|| format!("failed to list objects under {prefix}"))?;
        let body = success_text(response).await?;
        let parsed = ListObjectsV2::parse_response(&body)
            .context("failed to parse list-objects response")?;
        Ok(parsed
            .contents
            .into_iter()
            .map(|object| object.key)
            .filter(|key| key.ends_with(".tar.zst"))
            .collect())
    }

    /// Delete a single object, tolerating an already-absent key.
    ///
    /// # Errors
    ///
    /// Returns an error if the delete request fails at the transport level.
    pub(crate) async fn delete_object(&self, key: &str) -> Result<()> {
        let action = self.bucket.delete_object(Some(&self.credentials), key);
        let url = action.sign(SIGN_EXPIRY);
        let response = self
            .http
            .delete(url)
            .send()
            .await
            .with_context(|| format!("failed to delete {key}"))?;
        // S3 returns 204 for a delete, and treats a missing key as success too.
        success(response).await.map(drop)
    }
}

/// Return `response` when it carries a 2xx, else an error with the status and
/// (truncated) body for diagnosis.
async fn success(response: reqwest::Response) -> Result<reqwest::Response> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let body = response.text().await.unwrap_or_default();
    bail!("s3 request failed with status {status}: {}", body.trim());
}

/// Like [`success`] but returns the body text of a checked 2xx response.
async fn success_text(response: reqwest::Response) -> Result<String> {
    let response = success(response).await?;
    response
        .text()
        .await
        .context("failed to read s3 response body")
}
