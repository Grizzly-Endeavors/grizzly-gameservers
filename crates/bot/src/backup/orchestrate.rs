//! The backup/archive/restore/recover flows. Each composes existing Agones
//! lifecycle actions (`crate::agones`) with the supervisor's streaming archive
//! routes and the [`super::s3`] shell — the bot is the only thing that touches S3.

use std::time::Duration;

use anyhow::{Context, Result};
use grizzly_control_api::{ARCHIVE_PATH, ArchiveQuery, ControlError, ExtractQuery};
use jiff::Timestamp;
use tracing::{error, info, warn};

use super::manifest::{
    ArtifactKind, BackupManifest, CREATED_BY_AUTO, MANIFEST_SCHEMA, archive_keys, backup_keys,
    backup_prefix, keys_to_prune, manifest_key_for, stamp_now,
};
use super::s3::S3Store;
use super::store::{ArchiveRecord, ArchiveStore};
use super::{
    ArchiveOutcome, ArtifactSummary, BackupCtx, BackupOutcome, BackupService, RecoverOutcome,
    RestoreOutcome,
};
use crate::agones::{
    BackupTarget, ControlReady, DestroyOutcome, PodTarget, ProvisionOutcome, ReadyWait,
    RuntimeState, ServerScope, StartBegin, SupervisorOutcome, begin_start, destroy_instance,
    instance_runtime_state, list_backup_targets, provision_paused_instance, resolve_managed_pod,
    supervisor_start, supervisor_stop, wait_for_control_reachable, wait_for_ready,
};
use crate::config::{DbConfig, S3Config};

/// `created_by` recorded for a safety backup the restore path takes before it
/// overwrites a world, so an overwrite is always undoable.
const CREATED_BY_PRE_RESTORE: &str = "auto-pre-restore";

/// A streaming `GET /archive` against a supervisor, or why it couldn't be opened.
enum ArchiveSource {
    Ready(reqwest::Response),
    NotFound,
    NotManaged,
    PodNotReady,
    Unreachable(String),
}

/// The result of streaming an archive into a supervisor's `POST /archive`.
enum ExtractResult {
    Ok,
    NotFound,
    NotManaged,
    PodNotReady,
    Unreachable(String),
}

impl BackupService {
    /// Build the service from config. Connects the archive index (degrading to
    /// disabled if Postgres is down), but performs no S3 IO yet.
    ///
    /// # Errors
    ///
    /// Returns an error only if the S3 client can't be constructed from config
    /// (an invalid endpoint URL) — connectivity is checked lazily on first use.
    pub(crate) async fn new(
        s3_config: &S3Config,
        db_config: Option<&DbConfig>,
        retention: usize,
        interval: Duration,
    ) -> Result<Self> {
        // No global timeout: a multi-gigabyte world can stream for minutes. The
        // per-op work is bounded by the supervisor and S3 responding, not a clock.
        let stream_http = reqwest::Client::builder()
            .build()
            .map_err(|err| anyhow::anyhow!("failed to build the backup http client: {err}"))?;
        let s3 = S3Store::new(s3_config, stream_http.clone())?;
        let archives = ArchiveStore::connect(db_config).await;
        Ok(Self {
            s3,
            archives,
            stream_http,
            retention,
            interval,
        })
    }

    /// How often the scheduled backup cycle should run.
    pub(crate) fn interval(&self) -> Duration {
        self.interval
    }

    /// Snapshot every live managed server to S3, pruning each to the retention
    /// limit. Driven by the scheduled timer in `crate::run`; logs a summary once
    /// rather than a line per server.
    pub(crate) async fn run_backup_cycle(&self, ctx: &BackupCtx<'_>) {
        let targets = match list_backup_targets(ctx.client, ctx.namespace).await {
            Ok(targets) => targets,
            Err(err) => {
                error!(error = ?err, "scheduled backup cycle could not list servers");
                return;
            }
        };
        if targets.is_empty() {
            return;
        }
        info!(servers = targets.len(), "starting scheduled backup cycle");
        let (mut backed_up, mut failed) = (0_usize, 0_usize);
        for target in targets {
            match self.snapshot_target(ctx, &target, CREATED_BY_AUTO).await {
                Ok(BackupOutcome::BackedUp { .. }) => backed_up += 1,
                Ok(other) => {
                    failed += 1;
                    warn!(
                        instance = %target.instance,
                        outcome = ?other,
                        "scheduled backup did not complete"
                    );
                }
                Err(err) => {
                    failed += 1;
                    error!(error = ?err, "scheduled backup failed");
                }
            }
        }
        info!(backed_up, failed, "scheduled backup cycle complete");
    }

    /// Take one on-demand backup of a live server.
    ///
    /// # Errors
    ///
    /// Returns an error only on an unexpected cluster/S3 fault; expected outcomes
    /// (not found, not running) are values.
    pub(crate) async fn backup_instance(
        &self,
        ctx: &BackupCtx<'_>,
        instance: &str,
        created_by: &str,
    ) -> Result<BackupOutcome> {
        let Some(target) = self.find_target(ctx, instance).await? else {
            return Ok(
                match instance_runtime_state(ctx.client, ctx.namespace, instance).await? {
                    RuntimeState::Absent => BackupOutcome::NotFound,
                    RuntimeState::Down | RuntimeState::PodUp => BackupOutcome::NotRunning,
                },
            );
        };
        self.snapshot_target(ctx, &target, created_by).await
    }

    /// List a server's backups, newest first.
    ///
    /// # Errors
    ///
    /// Returns an error if the bucket can't be listed or a manifest can't be read.
    pub(crate) async fn list_backups(&self, instance: &str) -> Result<Vec<ArtifactSummary>> {
        let mut tarballs = self.s3.list_tarballs(&backup_prefix(instance)).await?;
        tarballs.sort();
        tarballs.reverse();
        self.summaries(tarballs).await
    }

    /// List the archives visible under `scope` (latest per name), newest first.
    /// A guild scope lists that guild's archives; the cross-guild operator scope
    /// lists every guild's.
    ///
    /// # Errors
    ///
    /// Returns an error if the archive catalog is disabled or the query fails.
    pub(crate) async fn list_archives(&self, scope: &ServerScope) -> Result<Vec<ArtifactSummary>> {
        let records = match scope {
            ServerScope::All => self.archives.list_all_latest_per_name().await?,
            ServerScope::Guild(guild) => self.archives.list_latest_per_name(guild).await?,
        };
        Ok(records
            .into_iter()
            .map(|record| ArtifactSummary {
                name: record.name,
                guild: record.guild,
                key: record.tarball_key,
                size_bytes: u64::try_from(record.size_bytes).unwrap_or(0),
                created_at: record.created_at,
            })
            .collect())
    }

    /// Whether the archive catalog (Postgres) is available — archive/recover need
    /// it, so the command layer checks this before offering them.
    pub(crate) fn archives_enabled(&self) -> bool {
        self.archives.enabled()
    }

    /// Stop a server, back it up to the archive area, record it, and release the
    /// whole trio (PVC included).
    ///
    /// # Errors
    ///
    /// Returns an error only on an unexpected cluster/S3/DB fault.
    pub(crate) async fn archive_instance(
        &self,
        ctx: &BackupCtx<'_>,
        instance: &str,
        created_by: &str,
    ) -> Result<ArchiveOutcome> {
        if !self.archives.enabled() {
            return Ok(ArchiveOutcome::Unavailable);
        }
        // Ensure a pod is up (cold-start a shut-down server) so its /data is
        // reachable to stream.
        match self.ensure_pod_reachable(ctx, instance).await? {
            EnsurePod::Ready => {}
            EnsurePod::NotFound => return Ok(ArchiveOutcome::NotFound),
            EnsurePod::NotManaged => return Ok(ArchiveOutcome::NotManaged),
            EnsurePod::Failed(reason) => return Ok(ArchiveOutcome::Failed(reason)),
        }
        let Some(target) = self.find_target(ctx, instance).await? else {
            return Ok(ArchiveOutcome::NotFound);
        };

        // Stop for a saved, consistent snapshot (SIGTERM flushes the world).
        if let Some(outcome) = stop_for_snapshot(ctx, instance).await? {
            return Ok(match outcome {
                StopBlock::NotFound => ArchiveOutcome::NotFound,
                StopBlock::NotManaged => ArchiveOutcome::NotManaged,
                StopBlock::Failed(reason) => ArchiveOutcome::Failed(reason),
            });
        }

        let guild = target.guild.clone();
        let stamp = stamp_now();
        let keys = archive_keys(&guild, instance, &stamp);
        let source = match self.open_archive_stream(ctx, instance, false).await? {
            ArchiveSource::Ready(response) => response,
            ArchiveSource::NotFound => return Ok(ArchiveOutcome::NotFound),
            ArchiveSource::NotManaged => return Ok(ArchiveOutcome::NotManaged),
            ArchiveSource::PodNotReady => {
                return Ok(ArchiveOutcome::Failed(
                    "the server pod wasn't ready".to_owned(),
                ));
            }
            ArchiveSource::Unreachable(reason) => return Ok(ArchiveOutcome::Failed(reason)),
        };
        let size = self
            .s3
            .upload_stream(&keys.tarball, source)
            .await
            .with_context(|| format!("failed to upload archive for {instance}"))?;
        let manifest = manifest(
            ArtifactKind::Archive,
            instance,
            &target.game,
            &guild,
            created_by,
            &keys.tarball,
            size,
        );
        self.s3
            .put_manifest(&keys.manifest, &manifest)
            .await
            .with_context(|| format!("failed to upload archive manifest for {instance}"))?;
        self.archives
            .insert(&ArchiveRecord {
                guild,
                name: instance.to_owned(),
                game: target.game.clone(),
                tarball_key: keys.tarball.clone(),
                manifest_key: keys.manifest.clone(),
                size_bytes: i64::try_from(size).unwrap_or(i64::MAX),
                created_by: created_by.to_owned(),
                created_at: String::new(),
            })
            .await
            .with_context(|| format!("failed to record archive for {instance}"))?;

        // The archive is durable in S3 + Postgres — now release the trio.
        match destroy_instance(ctx.client, ctx.namespace, instance).await? {
            DestroyOutcome::Destroyed | DestroyOutcome::NotFound => {}
            DestroyOutcome::NotManaged => {
                warn!(instance, "archived server was unmanaged at teardown");
            }
        }
        Ok(ArchiveOutcome::Archived {
            name: instance.to_owned(),
            size_bytes: size,
        })
    }

    /// Roll a live server back to one of its backups (`tarball_key`), taking a
    /// safety backup of the current world first so the overwrite is undoable.
    ///
    /// # Errors
    ///
    /// Returns an error only on an unexpected cluster/S3 fault.
    pub(crate) async fn restore_backup(
        &self,
        ctx: &BackupCtx<'_>,
        instance: &str,
        tarball_key: &str,
    ) -> Result<RestoreOutcome> {
        match self.ensure_pod_reachable(ctx, instance).await? {
            EnsurePod::Ready => {}
            EnsurePod::NotFound => return Ok(RestoreOutcome::NotFound),
            EnsurePod::NotManaged => return Ok(RestoreOutcome::NotManaged),
            EnsurePod::Failed(reason) => return Ok(RestoreOutcome::Failed(reason)),
        }

        // Best-effort safety net: snapshot the current world before overwriting it.
        if let Some(target) = self.find_target(ctx, instance).await?
            && let Err(err) = self
                .snapshot_target(ctx, &target, CREATED_BY_PRE_RESTORE)
                .await
        {
            warn!(error = ?err, instance, "pre-restore safety backup failed; continuing");
        }

        if let Some(block) = stop_for_snapshot(ctx, instance).await? {
            return Ok(match block {
                StopBlock::NotFound => RestoreOutcome::NotFound,
                StopBlock::NotManaged => RestoreOutcome::NotManaged,
                StopBlock::Failed(reason) => RestoreOutcome::Failed(reason),
            });
        }

        let download = self
            .s3
            .download_stream(tarball_key)
            .await
            .with_context(|| format!("failed to download backup {tarball_key}"))?;
        match self
            .push_archive_stream(ctx, instance, true, download)
            .await?
        {
            ExtractResult::Ok => {}
            ExtractResult::NotFound => return Ok(RestoreOutcome::NotFound),
            ExtractResult::NotManaged => return Ok(RestoreOutcome::NotManaged),
            ExtractResult::PodNotReady => {
                return Ok(RestoreOutcome::Failed(
                    "the server pod wasn't ready".to_owned(),
                ));
            }
            ExtractResult::Unreachable(reason) => return Ok(RestoreOutcome::Failed(reason)),
        }

        let ready = self.start_and_wait(ctx, instance).await?;
        Ok(match ready {
            StartResult::Ready(ready) => RestoreOutcome::Restored { ready },
            StartResult::Failed(reason) => RestoreOutcome::Failed(reason),
        })
    }

    /// Recover an archived server: recreate the trio held paused, reseed `/data`
    /// from the archive, then launch the game.
    ///
    /// # Errors
    ///
    /// Returns an error only on an unexpected cluster/S3/DB fault.
    pub(crate) async fn recover_archive(
        &self,
        ctx: &BackupCtx<'_>,
        guild: &str,
        name: &str,
    ) -> Result<RecoverOutcome> {
        if !self.archives.enabled() {
            return Ok(RecoverOutcome::Unavailable);
        }
        let Some(record) = self.archives.latest(guild, name).await? else {
            return Ok(RecoverOutcome::NoSuchArchive);
        };
        if !matches!(
            instance_runtime_state(ctx.client, ctx.namespace, name).await?,
            RuntimeState::Absent
        ) {
            return Ok(RecoverOutcome::NameInUse);
        }
        let Some(entry) = ctx.catalog.get(&record.game) else {
            return Ok(RecoverOutcome::UnknownGame(record.game));
        };

        // Stamp the recovered server with the archive's *own* owning guild, not
        // whatever guild the caller happens to be in — so a cross-guild operator
        // recovering an archive returns it to its original tenant.
        let address = match provision_paused_instance(
            ctx.client,
            ctx.namespace,
            ctx.domain,
            ctx.provision_lock,
            entry,
            name,
            &record.guild,
        )
        .await?
        {
            ProvisionOutcome::Provisioned { address } => address,
            ProvisionOutcome::AlreadyExists => return Ok(RecoverOutcome::NameInUse),
            ProvisionOutcome::PortsExhausted => return Ok(RecoverOutcome::PortsExhausted),
        };

        let outcome = self
            .seed_and_launch(ctx, name, &record.tarball_key, address)
            .await?;
        if let RecoverOutcome::Failed(_) = &outcome {
            // Free the name so a retry (or a different recovery) isn't blocked by a
            // half-built server; the archive itself is untouched.
            if let Err(err) = destroy_instance(ctx.client, ctx.namespace, name).await {
                warn!(error = ?err, name, "failed to clean up after a failed recover");
            }
        }
        Ok(outcome)
    }

    /// Seed a freshly provisioned (paused) server from its archive and launch it.
    async fn seed_and_launch(
        &self,
        ctx: &BackupCtx<'_>,
        name: &str,
        tarball_key: &str,
        address: String,
    ) -> Result<RecoverOutcome> {
        match wait_for_control_reachable(
            ctx.client,
            ctx.http,
            ctx.namespace,
            name,
            ctx.control_port,
        )
        .await?
        {
            ControlReady::Reachable => {}
            ControlReady::NotFound | ControlReady::NotManaged => {
                return Ok(RecoverOutcome::Failed(
                    "the recovered server disappeared before it could be seeded".to_owned(),
                ));
            }
            ControlReady::TimedOut => {
                return Ok(RecoverOutcome::Failed(
                    "the recovered server didn't come up in time".to_owned(),
                ));
            }
        }

        let download = self
            .s3
            .download_stream(tarball_key)
            .await
            .with_context(|| format!("failed to download archive {tarball_key}"))?;
        // Fresh PVC, so no purge needed before extracting.
        match self.push_archive_stream(ctx, name, false, download).await? {
            ExtractResult::Ok => {}
            ExtractResult::NotFound | ExtractResult::NotManaged | ExtractResult::PodNotReady => {
                return Ok(RecoverOutcome::Failed(
                    "the recovered server wasn't reachable to seed".to_owned(),
                ));
            }
            ExtractResult::Unreachable(reason) => return Ok(RecoverOutcome::Failed(reason)),
        }

        Ok(match self.start_and_wait(ctx, name).await? {
            StartResult::Ready(ready) => RecoverOutcome::Recovered { address, ready },
            StartResult::Failed(reason) => RecoverOutcome::Failed(reason),
        })
    }

    /// Snapshot one already-resolved live server to `backups/<instance>/`, then
    /// prune to the retention limit. Quiesces (flushes) only when the process is
    /// running — a paused server's `/data` is already saved and its RCON is down.
    async fn snapshot_target(
        &self,
        ctx: &BackupCtx<'_>,
        target: &BackupTarget,
        created_by: &str,
    ) -> Result<BackupOutcome> {
        let instance = &target.instance;
        let source = match self
            .open_archive_stream(ctx, instance, target.running)
            .await?
        {
            ArchiveSource::Ready(response) => response,
            ArchiveSource::NotFound => return Ok(BackupOutcome::NotFound),
            ArchiveSource::NotManaged => return Ok(BackupOutcome::NotManaged),
            ArchiveSource::PodNotReady => return Ok(BackupOutcome::NotRunning),
            ArchiveSource::Unreachable(reason) => return Ok(BackupOutcome::Unreachable(reason)),
        };
        let stamp = stamp_now();
        let keys = backup_keys(instance, &stamp);
        let size = self
            .s3
            .upload_stream(&keys.tarball, source)
            .await
            .with_context(|| format!("failed to upload backup for {instance}"))?;
        let manifest = manifest(
            ArtifactKind::Backup,
            instance,
            &target.game,
            &target.guild,
            created_by,
            &keys.tarball,
            size,
        );
        self.s3
            .put_manifest(&keys.manifest, &manifest)
            .await
            .with_context(|| format!("failed to upload backup manifest for {instance}"))?;
        self.prune_backups(instance).await;
        Ok(BackupOutcome::BackedUp { size_bytes: size })
    }

    /// Delete backups older than the newest `retention` under a server's prefix.
    /// Best-effort: a prune failure is logged, never fatal to the backup itself.
    async fn prune_backups(&self, instance: &str) {
        let tarballs = match self.s3.list_tarballs(&backup_prefix(instance)).await {
            Ok(tarballs) => tarballs,
            Err(err) => {
                warn!(error = ?err, instance, "could not list backups to prune");
                return;
            }
        };
        for key in keys_to_prune(tarballs, self.retention) {
            if let Err(err) = self.s3.delete_object(&key).await {
                warn!(error = ?err, key, "failed to prune old backup tarball");
                continue;
            }
            if let Some(manifest_key) = manifest_key_for(&key)
                && let Err(err) = self.s3.delete_object(&manifest_key).await
            {
                warn!(error = ?err, key = manifest_key, "failed to prune old backup manifest");
            }
        }
    }

    /// Read each tarball's manifest to build display summaries, newest-first order
    /// preserved from the caller.
    async fn summaries(&self, tarballs: Vec<String>) -> Result<Vec<ArtifactSummary>> {
        let mut summaries = Vec::with_capacity(tarballs.len());
        for tarball in tarballs {
            let Some(manifest_key) = manifest_key_for(&tarball) else {
                continue;
            };
            let manifest = self.s3.get_manifest(&manifest_key).await?;
            summaries.push(ArtifactSummary {
                name: manifest.instance,
                guild: manifest.guild,
                key: tarball,
                size_bytes: manifest.size_bytes,
                created_at: manifest.created_at,
            });
        }
        Ok(summaries)
    }

    /// Open a streaming `GET /archive` against an instance's supervisor.
    async fn open_archive_stream(
        &self,
        ctx: &BackupCtx<'_>,
        instance: &str,
        quiesce: bool,
    ) -> Result<ArchiveSource> {
        let pod_ip = match resolve_managed_pod(ctx.client, ctx.namespace, instance).await? {
            PodTarget::Ready(pod_ip) => pod_ip,
            PodTarget::NotFound => return Ok(ArchiveSource::NotFound),
            PodTarget::NotManaged => return Ok(ArchiveSource::NotManaged),
            PodTarget::PodNotReady => return Ok(ArchiveSource::PodNotReady),
        };
        let url = format!("http://{pod_ip}:{}{ARCHIVE_PATH}", ctx.control_port);
        match self
            .stream_http
            .get(&url)
            .query(&ArchiveQuery { quiesce })
            .send()
            .await
        {
            Ok(response) if response.status().is_success() => Ok(ArchiveSource::Ready(response)),
            Ok(response) => Ok(ArchiveSource::Unreachable(
                control_error(response, "archive").await,
            )),
            Err(err) => {
                warn!(error = ?err, url, "failed to open the archive stream");
                Ok(ArchiveSource::Unreachable(
                    "couldn't reach the server to back it up".to_owned(),
                ))
            }
        }
    }

    /// Stream `source` into an instance's supervisor `POST /archive`, purging its
    /// data root first when `purge` is set (overwrite-restore).
    async fn push_archive_stream(
        &self,
        ctx: &BackupCtx<'_>,
        instance: &str,
        purge: bool,
        source: reqwest::Response,
    ) -> Result<ExtractResult> {
        let pod_ip = match resolve_managed_pod(ctx.client, ctx.namespace, instance).await? {
            PodTarget::Ready(pod_ip) => pod_ip,
            PodTarget::NotFound => return Ok(ExtractResult::NotFound),
            PodTarget::NotManaged => return Ok(ExtractResult::NotManaged),
            PodTarget::PodNotReady => return Ok(ExtractResult::PodNotReady),
        };
        let url = format!("http://{pod_ip}:{}{ARCHIVE_PATH}", ctx.control_port);
        let body = reqwest::Body::wrap_stream(source.bytes_stream());
        match self
            .stream_http
            .post(&url)
            .query(&ExtractQuery { purge })
            .body(body)
            .send()
            .await
        {
            Ok(response) if response.status().is_success() => Ok(ExtractResult::Ok),
            Ok(response) => Ok(ExtractResult::Unreachable(
                control_error(response, "restore").await,
            )),
            Err(err) => {
                warn!(error = ?err, url, "failed to stream the archive into the server");
                Ok(ExtractResult::Unreachable(
                    "couldn't send the backup data to the server".to_owned(),
                ))
            }
        }
    }

    /// Warm-start a server and wait for it to accept players again.
    async fn start_and_wait(&self, ctx: &BackupCtx<'_>, instance: &str) -> Result<StartResult> {
        match supervisor_start(
            ctx.client,
            ctx.http,
            ctx.namespace,
            instance,
            ctx.control_port,
        )
        .await?
        {
            SupervisorOutcome::Resumed | SupervisorOutcome::AlreadyRunning => {}
            SupervisorOutcome::Failed(reason) => return Ok(StartResult::Failed(reason)),
            SupervisorOutcome::Paused
            | SupervisorOutcome::Restarted
            | SupervisorOutcome::AlreadyStopped
            | SupervisorOutcome::PodNotReady
            | SupervisorOutcome::Unreachable
            | SupervisorOutcome::NotFound
            | SupervisorOutcome::NotManaged => {
                return Ok(StartResult::Failed(
                    "the server couldn't be started after the restore".to_owned(),
                ));
            }
        }
        let ready = matches!(
            wait_for_ready(
                ctx.client,
                ctx.http,
                ctx.namespace,
                instance,
                ctx.control_port
            )
            .await?,
            ReadyWait::Ready
        );
        Ok(StartResult::Ready(ready))
    }

    /// Ensure an instance has a live pod, cold-starting a shut-down one.
    async fn ensure_pod_reachable(&self, ctx: &BackupCtx<'_>, instance: &str) -> Result<EnsurePod> {
        match instance_runtime_state(ctx.client, ctx.namespace, instance).await? {
            RuntimeState::Absent => return Ok(EnsurePod::NotFound),
            RuntimeState::PodUp => return Ok(EnsurePod::Ready),
            RuntimeState::Down => {}
        }
        match begin_start(ctx.client, ctx.namespace, ctx.domain, ctx.catalog, instance).await? {
            StartBegin::Starting { .. } | StartBegin::AlreadyRunning => {}
            StartBegin::NotFound => return Ok(EnsurePod::NotFound),
            StartBegin::NotManaged => return Ok(EnsurePod::NotManaged),
            StartBegin::UnknownGame(game) => {
                return Ok(EnsurePod::Failed(format!(
                    "the server's game ({game}) is no longer in the catalog"
                )));
            }
        }
        Ok(
            match wait_for_control_reachable(
                ctx.client,
                ctx.http,
                ctx.namespace,
                instance,
                ctx.control_port,
            )
            .await?
            {
                ControlReady::Reachable => EnsurePod::Ready,
                ControlReady::NotFound => EnsurePod::NotFound,
                ControlReady::NotManaged => EnsurePod::NotManaged,
                ControlReady::TimedOut => {
                    EnsurePod::Failed("the server didn't come up in time".to_owned())
                }
            },
        )
    }

    /// Look up a live server's backup target (game/guild/running) by name.
    async fn find_target(
        &self,
        ctx: &BackupCtx<'_>,
        instance: &str,
    ) -> Result<Option<BackupTarget>> {
        let targets = list_backup_targets(ctx.client, ctx.namespace).await?;
        Ok(targets
            .into_iter()
            .find(|target| target.instance == instance))
    }
}

/// The result of ensuring a server has a reachable pod.
enum EnsurePod {
    Ready,
    NotFound,
    NotManaged,
    Failed(String),
}

/// A non-happy result from [`stop_for_snapshot`].
enum StopBlock {
    NotFound,
    NotManaged,
    Failed(String),
}

/// The result of starting a server and waiting for readiness.
enum StartResult {
    Ready(bool),
    Failed(String),
}

/// Stop a server's process so its `/data` is flushed and consistent before a
/// snapshot. Returns `None` when the stop landed (paused or already stopped), or
/// the blocking reason otherwise.
async fn stop_for_snapshot(ctx: &BackupCtx<'_>, instance: &str) -> Result<Option<StopBlock>> {
    Ok(
        match supervisor_stop(
            ctx.client,
            ctx.http,
            ctx.namespace,
            instance,
            ctx.control_port,
        )
        .await?
        {
            SupervisorOutcome::Paused | SupervisorOutcome::AlreadyStopped => None,
            SupervisorOutcome::NotFound => Some(StopBlock::NotFound),
            SupervisorOutcome::NotManaged => Some(StopBlock::NotManaged),
            SupervisorOutcome::PodNotReady => {
                Some(StopBlock::Failed("the server isn't ready yet".to_owned()))
            }
            SupervisorOutcome::Unreachable => {
                Some(StopBlock::Failed("couldn't reach the server".to_owned()))
            }
            SupervisorOutcome::Failed(reason) => Some(StopBlock::Failed(reason)),
            SupervisorOutcome::Resumed
            | SupervisorOutcome::Restarted
            | SupervisorOutcome::AlreadyRunning => {
                Some(StopBlock::Failed("unexpected stop result".to_owned()))
            }
        },
    )
}

/// Build a manifest for a just-uploaded artifact.
fn manifest(
    kind: ArtifactKind,
    instance: &str,
    game: &str,
    guild: &str,
    created_by: &str,
    tarball_key: &str,
    size_bytes: u64,
) -> BackupManifest {
    BackupManifest {
        schema: MANIFEST_SCHEMA,
        kind,
        instance: instance.to_owned(),
        game: game.to_owned(),
        guild: guild.to_owned(),
        created_by: created_by.to_owned(),
        created_at: Timestamp::now().to_string(),
        tarball_key: tarball_key.to_owned(),
        size_bytes,
    }
}

/// Read a supervisor error reply's [`ControlError`] message for logging/relay,
/// falling back to the HTTP status when the body isn't parseable.
async fn control_error(response: reqwest::Response, op: &str) -> String {
    let status = response.status();
    match response.json::<ControlError>().await {
        Ok(error) => {
            warn!(%status, op, error = error.error, "supervisor archive route refused the request");
            error.error
        }
        Err(err) => {
            warn!(%status, op, error = ?err, "supervisor archive route returned an unreadable error");
            format!("the server rejected the {op} (status {status})")
        }
    }
}
