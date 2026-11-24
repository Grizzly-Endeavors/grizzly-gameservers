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
    ArchiveDeleteOutcome, ArchiveOutcome, ArtifactSummary, BackupCtx, BackupOutcome, BackupService,
    BootState, RecoverOutcome, RestoreOutcome, SafetyBackup,
};
use crate::agones::{
    ControlReady, DestroyOutcome, LiveInstance, PodTarget, ProvisionOutcome, ProvisionSpec,
    ReadyWait, RuntimeState, ServerScope, StartBegin, SupervisorOutcome, begin_start,
    destroy_instance, instance_runtime_state, list_live_instances, provision_paused_instance,
    resolve_managed_pod, supervisor_start, supervisor_stop, wait_for_control_reachable,
    wait_for_ready,
};
use crate::agones::{InstanceOwner, owner_of};
use crate::config::{DbConfig, S3Config};
use crate::domain::{GameId, GuildId, InstanceName};
use crate::events::{Attribution, EventKind, ProductEvent, mapping};

const CREATED_BY_PRE_RESTORE: &str = "auto-pre-restore";

const BACKUP_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const BACKUP_READ_TIMEOUT: Duration = Duration::from_mins(5);

enum ArchiveSource {
    Ready(reqwest::Response),
    NotFound,
    NotManaged,
    PodNotReady,
    Unreachable(String),
}

enum ExtractResult {
    Ok,
    NotFound,
    NotManaged,
    PodNotReady,
    Unreachable(String),
}

impl BackupService {
    pub(crate) async fn new(
        s3_config: &S3Config,
        db_config: Option<&DbConfig>,
        retention: usize,
        archive_retention_days: i32,
        supervisor_token: Option<&str>,
        interval: Duration,
        events: std::sync::Arc<crate::events::EventLog>,
    ) -> Result<Self> {
        let stream_http = reqwest::Client::builder()
            .connect_timeout(BACKUP_CONNECT_TIMEOUT)
            .read_timeout(BACKUP_READ_TIMEOUT)
            .build()
            .map_err(|err| anyhow::anyhow!("failed to build the backup http client: {err}"))?;
        let s3 = S3Store::new(s3_config, stream_http.clone())?;
        let archive_http = crate::agones::ControlClient::new(stream_http.clone(), supervisor_token);
        let archives = ArchiveStore::connect(db_config).await;
        Ok(Self {
            s3,
            archives,
            archive_http,
            retention,
            archive_retention_days,
            interval,
            events,
        })
    }

    async fn record(&self, ctx: &BackupCtx<'_>, event: ProductEvent) {
        self.events
            .record_lifecycle(ctx.client, ctx.namespace, event)
            .await;
    }

    pub(crate) fn interval(&self) -> Duration {
        self.interval
    }

    pub(crate) async fn run_backup_cycle(&self, ctx: &BackupCtx<'_>) {
        let targets = match list_live_instances(ctx.client, ctx.namespace).await {
            Ok(targets) => targets,
            Err(err) => {
                error!(error = ?err, "scheduled backup cycle could not list servers");
                return;
            }
        };
        let servers = targets.len();
        if targets.is_empty() {
            self.record_cycle(servers, 0, 0).await;
            self.prune_archives(ctx).await;
            return;
        }
        info!(servers, "starting scheduled backup cycle");
        let attribution = Attribution::system();
        let (mut backed_up, mut failed) = (0_usize, 0_usize);
        for target in targets {
            match self.snapshot_target(ctx, &target, CREATED_BY_AUTO).await {
                Ok(outcome) => {
                    self.record(
                        ctx,
                        Self::snapshot_event(
                            EventKind::BackupTaken,
                            &attribution,
                            &target,
                            &outcome,
                        ),
                    )
                    .await;
                    if matches!(outcome, BackupOutcome::BackedUp { .. }) {
                        backed_up += 1;
                    } else {
                        failed += 1;
                        warn!(
                            instance = %target.instance,
                            outcome = ?outcome,
                            "scheduled backup did not complete"
                        );
                    }
                }
                Err(err) => {
                    failed += 1;
                    error!(
                        error = ?err,
                        instance = %target.instance,
                        "scheduled backup failed"
                    );
                }
            }
        }
        info!(backed_up, failed, "scheduled backup cycle complete");
        self.record_cycle(servers, backed_up, failed).await;
        self.prune_archives(ctx).await;
    }

    async fn record_cycle(&self, servers: usize, succeeded: usize, failed: usize) {
        self.events
            .record(
                ProductEvent::new(
                    EventKind::BackupCycle,
                    &Attribution::system(),
                    mapping::COMPLETED,
                )
                .details(mapping::cycle_details(servers, succeeded, failed)),
            )
            .await;
    }

    pub(crate) async fn backup_instance(
        &self,
        ctx: &BackupCtx<'_>,
        instance: &str,
        attribution: &Attribution,
    ) -> Result<BackupOutcome> {
        let Some(target) = self.find_target(ctx, instance).await? else {
            let outcome = match instance_runtime_state(ctx.client, ctx.namespace, instance).await? {
                RuntimeState::Absent => BackupOutcome::NotFound,
                RuntimeState::Down | RuntimeState::PodUp | RuntimeState::Terminating => {
                    BackupOutcome::NotRunning
                }
            };
            self.record(
                ctx,
                ProductEvent::new(
                    EventKind::BackupTaken,
                    attribution,
                    mapping::backup(&outcome),
                )
                .instance(InstanceName::new(instance)),
            )
            .await;
            return Ok(outcome);
        };
        let outcome = self
            .snapshot_target(ctx, &target, &attribution.actor.label())
            .await?;
        self.record(
            ctx,
            Self::snapshot_event(EventKind::BackupTaken, attribution, &target, &outcome),
        )
        .await;
        Ok(outcome)
    }

    fn snapshot_event(
        kind: EventKind,
        attribution: &Attribution,
        target: &LiveInstance,
        outcome: &BackupOutcome,
    ) -> ProductEvent {
        let event = ProductEvent::new(kind, attribution, mapping::backup(outcome))
            .instance(target.instance.clone())
            .guild(Some(target.guild.clone()))
            .game(Some(target.game.clone()));
        match outcome {
            BackupOutcome::BackedUp { size_bytes } => {
                event.details(mapping::size_details(*size_bytes))
            }
            BackupOutcome::NotFound
            | BackupOutcome::NotManaged
            | BackupOutcome::NotRunning
            | BackupOutcome::Unreachable(_) => event,
        }
    }

    pub(crate) async fn list_backups(
        &self,
        ctx: &BackupCtx<'_>,
        instance: &str,
    ) -> Result<Vec<ArtifactSummary>> {
        let Some(guild) = self.owning_guild(ctx, instance).await? else {
            return Ok(Vec::new());
        };
        let mut tarballs = self
            .s3
            .list_tarballs(&backup_prefix(&guild, instance))
            .await?;
        tarballs.sort();
        tarballs.reverse();
        self.summaries(tarballs).await
    }

    async fn owning_guild(&self, ctx: &BackupCtx<'_>, instance: &str) -> Result<Option<String>> {
        let owner = owner_of(ctx.client, ctx.namespace, instance).await?;
        Ok(owner.map(|owner| owner.guild.map(GuildId::into_string).unwrap_or_default()))
    }

    pub(crate) async fn list_archives(&self, scope: &ServerScope) -> Result<Vec<ArtifactSummary>> {
        let records = match scope {
            ServerScope::All => self.archives.list_all_latest_per_name().await?,
            ServerScope::Guild(guild) => self.archives.list_latest_per_name(guild).await?,
        };
        Ok(records
            .into_iter()
            .map(|record| ArtifactSummary {
                name: record.name.into_string(),
                guild: record.guild.into_string(),
                key: record.tarball_key,
                size_bytes: u64::try_from(record.size_bytes).unwrap_or(0),
                created_at: record.created_at,
            })
            .collect())
    }

    pub(crate) fn archives_enabled(&self) -> bool {
        self.archives.enabled()
    }

    pub(crate) async fn delete_archives(
        &self,
        ctx: &BackupCtx<'_>,
        scope: &ServerScope,
        name: &str,
        attribution: &Attribution,
    ) -> Result<ArchiveDeleteOutcome> {
        if !self.archives.enabled() {
            return Ok(ArchiveDeleteOutcome::Unavailable);
        }
        let Some(guild) = self.guild_owning_archive(scope, name).await? else {
            return Ok(ArchiveDeleteOutcome::NotFound);
        };
        let records = self.archives.rows_for_name(guild.as_str(), name).await?;
        if records.is_empty() {
            return Ok(ArchiveDeleteOutcome::NotFound);
        }

        let game = records.first().map(|record| record.game.clone());
        let freed_bytes = records
            .iter()
            .map(|record| u64::try_from(record.size_bytes).unwrap_or(0))
            .sum();
        let generations = records.len();

        for record in &records {
            for key in [&record.tarball_key, &record.manifest_key] {
                if let Err(err) = self.s3.delete_object(key).await {
                    error!(error = ?err, key, name, "failed to delete an archive object");
                    self.record(
                        ctx,
                        ProductEvent::new(EventKind::ArchiveDeleted, attribution, mapping::FAILED)
                            .guild(Some(guild.clone()))
                            .game(game.clone())
                            .instance(InstanceName::new(name)),
                    )
                    .await;
                    return Ok(ArchiveDeleteOutcome::Failed(format!(
                        "could not delete archive object {key}: {err}"
                    )));
                }
            }
        }
        self.archives.delete_name(guild.as_str(), name).await?;

        self.record(
            ctx,
            ProductEvent::new(EventKind::ArchiveDeleted, attribution, mapping::DELETED)
                .guild(Some(guild.clone()))
                .game(game)
                .instance(InstanceName::new(name))
                .details(serde_json::json!({
                    "generations": generations,
                    "freed_bytes": freed_bytes,
                })),
        )
        .await;

        Ok(ArchiveDeleteOutcome::Deleted {
            name: name.to_owned(),
            generations,
            freed_bytes,
        })
    }

    pub(crate) async fn archive_footprint(
        &self,
        scope: &ServerScope,
        name: &str,
    ) -> Result<Option<(usize, u64)>> {
        let Some(guild) = self.guild_owning_archive(scope, name).await? else {
            return Ok(None);
        };
        let records = self.archives.rows_for_name(guild.as_str(), name).await?;
        if records.is_empty() {
            return Ok(None);
        }
        let total = records
            .iter()
            .map(|record| u64::try_from(record.size_bytes).unwrap_or(0))
            .sum();
        Ok(Some((records.len(), total)))
    }

    async fn guild_owning_archive(
        &self,
        scope: &ServerScope,
        name: &str,
    ) -> Result<Option<GuildId>> {
        match scope {
            ServerScope::Guild(guild) => Ok(self
                .archives
                .latest(guild, name)
                .await?
                .map(|record| record.guild)),
            ServerScope::All => Ok(self
                .archives
                .list_all_latest_per_name()
                .await?
                .into_iter()
                .find(|record| record.name.as_str() == name)
                .map(|record| record.guild)),
        }
    }

    async fn prune_archives(&self, ctx: &BackupCtx<'_>) {
        if !self.archives.enabled() {
            return;
        }
        let stale = match self
            .archives
            .rows_to_prune(self.archive_retention_days)
            .await
        {
            Ok(stale) => stale,
            Err(err) => {
                warn!(error = ?err, "could not list archives to prune");
                return;
            }
        };
        let mut pruned = 0_usize;
        for record in stale {
            let mut objects_gone = true;
            for key in [&record.tarball_key, &record.manifest_key] {
                if let Err(err) = self.s3.delete_object(key).await {
                    warn!(error = ?err, key, "failed to prune an aged archive object");
                    objects_gone = false;
                }
            }
            if !objects_gone {
                continue;
            }
            if let Err(err) = self.archives.delete_by_key(&record.tarball_key).await {
                warn!(error = ?err, key = record.tarball_key, "failed to drop a pruned archive row");
                continue;
            }
            pruned += 1;
            self.record(
                ctx,
                ProductEvent::new(
                    EventKind::ArchiveDeleted,
                    &Attribution::system(),
                    mapping::PRUNED,
                )
                .guild(Some(record.guild))
                .game(Some(record.game))
                .instance(record.name),
            )
            .await;
        }
        if pruned > 0 {
            info!(
                pruned,
                days = self.archive_retention_days,
                "pruned archives past the retention window"
            );
        }
    }

    pub(crate) async fn archive_instance(
        &self,
        ctx: &BackupCtx<'_>,
        instance: &str,
        attribution: &Attribution,
    ) -> Result<ArchiveOutcome> {
        let owner = owner_before(ctx, instance).await;
        let archived = self
            .run_archive(ctx, instance, &attribution.actor.label())
            .await;
        let event = ProductEvent::new(
            EventKind::ServerArchived,
            attribution,
            archived
                .as_ref()
                .map_or(mapping::FAILED, |outcome| mapping::archive(outcome)),
        )
        .instance(InstanceName::new(instance))
        .guild(owner.guild)
        .game(owner.game);
        let event = match archived.as_ref() {
            Ok(
                ArchiveOutcome::Archived { size_bytes, .. }
                | ArchiveOutcome::ArchivedNotReleased { size_bytes, .. },
            ) => event.details(mapping::size_details(*size_bytes)),
            Ok(
                ArchiveOutcome::NotFound
                | ArchiveOutcome::NotManaged
                | ArchiveOutcome::Unavailable
                | ArchiveOutcome::Failed(_),
            )
            | Err(_) => event,
        };
        self.record(ctx, event).await;
        archived
    }

    async fn run_archive(
        &self,
        ctx: &BackupCtx<'_>,
        instance: &str,
        created_by: &str,
    ) -> Result<ArchiveOutcome> {
        if !self.archives.enabled() {
            return Ok(ArchiveOutcome::Unavailable);
        }
        match self.ensure_pod_reachable(ctx, instance).await? {
            EnsurePod::Ready => {}
            EnsurePod::NotFound => return Ok(ArchiveOutcome::NotFound),
            EnsurePod::NotManaged => return Ok(ArchiveOutcome::NotManaged),
            EnsurePod::Failed(reason) => return Ok(ArchiveOutcome::Failed(reason)),
        }
        let Some(target) = self.find_target(ctx, instance).await? else {
            return Ok(ArchiveOutcome::NotFound);
        };

        if let Some(outcome) = stop_for_snapshot(ctx, instance).await? {
            return Ok(match outcome {
                StopBlock::NotFound => ArchiveOutcome::NotFound,
                StopBlock::NotManaged => ArchiveOutcome::NotManaged,
                StopBlock::Failed(reason) => ArchiveOutcome::Failed(reason),
            });
        }

        let guild = target.guild.clone();
        let stamp = stamp_now();
        let keys = archive_keys(guild.as_str(), instance, &stamp);
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
            &target.instance,
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
                name: target.instance.clone(),
                game: target.game.clone(),
                tarball_key: keys.tarball.clone(),
                manifest_key: keys.manifest.clone(),
                size_bytes: i64::try_from(size).unwrap_or(i64::MAX),
                created_by: created_by.to_owned(),
                created_at: String::new(),
            })
            .await
            .with_context(|| format!("failed to record archive for {instance}"))?;

        match destroy_instance(ctx.client, ctx.namespace, instance).await {
            Ok(DestroyOutcome::Destroyed | DestroyOutcome::NotFound) => {}
            Ok(DestroyOutcome::NotManaged) => {
                warn!(instance, "archived server was unmanaged at teardown");
            }
            Ok(DestroyOutcome::Blocked) => {
                warn!(
                    instance,
                    "archive committed but the server wouldn't confirm shut down; \
                     its storage was left in place and is safe to release later"
                );
                return Ok(ArchiveOutcome::ArchivedNotReleased {
                    name: instance.to_owned(),
                    size_bytes: size,
                });
            }
            Err(err) => {
                error!(
                    error = ?err,
                    instance,
                    "archive committed but releasing the server's storage failed; \
                     it's recoverable and safe to retry"
                );
                return Ok(ArchiveOutcome::ArchivedNotReleased {
                    name: instance.to_owned(),
                    size_bytes: size,
                });
            }
        }
        Ok(ArchiveOutcome::Archived {
            name: instance.to_owned(),
            size_bytes: size,
        })
    }

    pub(crate) async fn restore_backup(
        &self,
        ctx: &BackupCtx<'_>,
        instance: &str,
        tarball_key: &str,
        attribution: &Attribution,
    ) -> Result<RestoreOutcome> {
        let restored = self.run_restore(ctx, instance, tarball_key).await;
        let event = ProductEvent::new(
            EventKind::ServerRestored,
            attribution,
            restored
                .as_ref()
                .map_or(mapping::FAILED, |outcome| mapping::restore(outcome)),
        )
        .instance(InstanceName::new(instance));
        let event = match restored.as_ref() {
            Ok(RestoreOutcome::Restored {
                boot,
                safety_backup,
            }) => event.details(mapping::restore_details(*boot, *safety_backup)),
            Ok(
                RestoreOutcome::NotFound | RestoreOutcome::NotManaged | RestoreOutcome::Failed(_),
            )
            | Err(_) => event,
        };
        self.record(ctx, event).await;
        restored
    }

    async fn run_restore(
        &self,
        ctx: &BackupCtx<'_>,
        instance: &str,
        tarball_key: &str,
    ) -> Result<RestoreOutcome> {
        let owner = self.owning_guild(ctx, instance).await?;
        let expected = owner
            .as_deref()
            .map(|guild| backup_prefix(guild, instance))
            .unwrap_or_default();
        if expected.is_empty() || !tarball_key.starts_with(&expected) {
            warn!(
                instance,
                tarball_key,
                "refused a restore from a key outside the instance's own backup prefix"
            );
            return Ok(RestoreOutcome::NotFound);
        }

        match self.ensure_pod_reachable(ctx, instance).await? {
            EnsurePod::Ready => {}
            EnsurePod::NotFound => return Ok(RestoreOutcome::NotFound),
            EnsurePod::NotManaged => return Ok(RestoreOutcome::NotManaged),
            EnsurePod::Failed(reason) => return Ok(RestoreOutcome::Failed(reason)),
        }

        let safety_backup = if let Some(target) = self.find_target(ctx, instance).await? {
            match self
                .snapshot_target(ctx, &target, CREATED_BY_PRE_RESTORE)
                .await
            {
                Ok(BackupOutcome::BackedUp { .. }) => SafetyBackup::Taken,
                Ok(other) => {
                    warn!(
                        instance,
                        outcome = ?other,
                        "pre-restore safety backup didn't complete; restoring anyway with no undo point"
                    );
                    SafetyBackup::Absent
                }
                Err(err) => {
                    warn!(
                        error = ?err,
                        instance,
                        "pre-restore safety backup failed; restoring anyway with no undo point"
                    );
                    SafetyBackup::Absent
                }
            }
        } else {
            warn!(
                instance,
                "pre-restore safety backup skipped: no live backup target found; \
                 restoring anyway with no undo point"
            );
            SafetyBackup::Absent
        };

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

        Ok(match self.start_and_wait(ctx, instance).await? {
            StartResult::Ready(boot) => RestoreOutcome::Restored {
                boot,
                safety_backup,
            },
            StartResult::Failed(reason) => RestoreOutcome::Failed(reason),
        })
    }

    pub(crate) async fn recover_archive(
        &self,
        ctx: &BackupCtx<'_>,
        guild: &str,
        name: &str,
        attribution: &Attribution,
    ) -> Result<RecoverOutcome> {
        let recovered = self.run_recover(ctx, guild, name).await;
        let event = ProductEvent::new(
            EventKind::ServerRecovered,
            attribution,
            recovered
                .as_ref()
                .map_or(mapping::FAILED, |outcome| mapping::recover(outcome)),
        )
        .instance(InstanceName::new(name));
        let event = match recovered.as_ref() {
            Ok(RecoverOutcome::Recovered { address, boot }) => {
                event.details(mapping::recover_details(address, *boot))
            }
            Ok(
                RecoverOutcome::NoSuchArchive
                | RecoverOutcome::NameInUse
                | RecoverOutcome::Unavailable
                | RecoverOutcome::UnknownGame(_)
                | RecoverOutcome::PortsExhausted
                | RecoverOutcome::GuildLimitReached(_)
                | RecoverOutcome::ClusterFull(_)
                | RecoverOutcome::Failed(_),
            )
            | Err(_) => event,
        };
        self.record(ctx, event).await;
        recovered
    }

    async fn run_recover(
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
        let Some(entry) = ctx.catalog.get(record.game.as_str()) else {
            return Ok(RecoverOutcome::UnknownGame(record.game.into_string()));
        };

        let caps = ctx.guild_limits.caps_for(record.guild.as_str()).await;
        let address = match provision_paused_instance(
            ctx.client,
            ctx.namespace,
            ctx.domain,
            ctx.provision_lock,
            ProvisionSpec {
                entry,
                instance: name,
                guild: record.guild.as_str(),
                caps: Some(&caps),
            },
        )
        .await?
        {
            ProvisionOutcome::Provisioned { address } => address,
            ProvisionOutcome::AlreadyExists | ProvisionOutcome::NameUnavailable => {
                return Ok(RecoverOutcome::NameInUse);
            }
            ProvisionOutcome::PortsExhausted => return Ok(RecoverOutcome::PortsExhausted),
            ProvisionOutcome::GuildLimitReached(refusal) => {
                return Ok(RecoverOutcome::GuildLimitReached(refusal));
            }
            ProvisionOutcome::ClusterFull(refusal) => {
                return Ok(RecoverOutcome::ClusterFull(refusal));
            }
        };

        let outcome = self
            .seed_and_launch(ctx, name, &record.tarball_key, address)
            .await?;
        if let RecoverOutcome::Failed(_) = &outcome
            && let Err(err) = destroy_instance(ctx.client, ctx.namespace, name).await
        {
            warn!(error = ?err, name, "failed to clean up after a failed recover");
        }
        Ok(outcome)
    }

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
            StartResult::Ready(boot) => RecoverOutcome::Recovered { address, boot },
            StartResult::Failed(reason) => RecoverOutcome::Failed(reason),
        })
    }

    async fn snapshot_target(
        &self,
        ctx: &BackupCtx<'_>,
        target: &LiveInstance,
        created_by: &str,
    ) -> Result<BackupOutcome> {
        let instance = target.instance.as_str();
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
        let keys = backup_keys(target.guild.as_str(), instance, &stamp);
        let size = self
            .s3
            .upload_stream(&keys.tarball, source)
            .await
            .with_context(|| format!("failed to upload backup for {instance}"))?;
        let manifest = manifest(
            ArtifactKind::Backup,
            &target.instance,
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
        self.prune_backups(target.guild.as_str(), instance).await;
        Ok(BackupOutcome::BackedUp { size_bytes: size })
    }

    async fn prune_backups(&self, guild: &str, instance: &str) {
        let tarballs = match self.s3.list_tarballs(&backup_prefix(guild, instance)).await {
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

    async fn summaries(&self, tarballs: Vec<String>) -> Result<Vec<ArtifactSummary>> {
        let mut summaries = Vec::with_capacity(tarballs.len());
        for tarball in tarballs {
            let Some(manifest_key) = manifest_key_for(&tarball) else {
                continue;
            };
            let manifest = match self.s3.get_manifest(&manifest_key).await {
                Ok(manifest) => manifest,
                Err(err) => {
                    warn!(
                        error = ?err,
                        manifest_key,
                        tarball,
                        "skipping a backup whose manifest couldn't be read"
                    );
                    continue;
                }
            };
            summaries.push(ArtifactSummary {
                name: manifest.instance.into_string(),
                guild: manifest.guild.into_string(),
                key: tarball,
                size_bytes: manifest.size_bytes,
                created_at: manifest.created_at,
            });
        }
        Ok(summaries)
    }

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
            .archive_http
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
            .archive_http
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
        let boot = match wait_for_ready(
            ctx.client,
            ctx.http,
            ctx.namespace,
            instance,
            ctx.control_port,
        )
        .await?
        {
            ReadyWait::Ready => BootState::Ready,
            ReadyWait::Crashed => BootState::Crashed,
            ReadyWait::Stopped => BootState::Stopped,
            ReadyWait::TimedOut => BootState::TimedOut,
            ReadyWait::NotFound | ReadyWait::NotManaged => {
                return Ok(StartResult::Failed(
                    "the server disappeared while waiting for it to come back up".to_owned(),
                ));
            }
        };
        Ok(StartResult::Ready(boot))
    }

    async fn ensure_pod_reachable(&self, ctx: &BackupCtx<'_>, instance: &str) -> Result<EnsurePod> {
        match instance_runtime_state(ctx.client, ctx.namespace, instance).await? {
            RuntimeState::Absent => return Ok(EnsurePod::NotFound),
            RuntimeState::PodUp => return Ok(EnsurePod::Ready),
            RuntimeState::Terminating => {
                return Ok(EnsurePod::Failed(
                    "the server is still shutting down; try again in a few minutes".to_owned(),
                ));
            }
            RuntimeState::Down => {}
        }
        match begin_start(
            ctx.client,
            ctx.namespace,
            ctx.domain,
            ctx.catalog,
            instance,
            ctx.sizing.get(instance).await,
        )
        .await?
        {
            StartBegin::Starting { .. } | StartBegin::AlreadyRunning => {}
            StartBegin::NotFound => return Ok(EnsurePod::NotFound),
            StartBegin::NotManaged => return Ok(EnsurePod::NotManaged),
            StartBegin::UnknownGame(game) => {
                return Ok(EnsurePod::Failed(format!(
                    "the server's game ({game}) is no longer in the catalog"
                )));
            }
            StartBegin::ClusterFull(refusal) => {
                return Ok(EnsurePod::Failed(format!(
                    "there is no room on the hardware to start it ({})",
                    refusal.summary()
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

    async fn find_target(
        &self,
        ctx: &BackupCtx<'_>,
        instance: &str,
    ) -> Result<Option<LiveInstance>> {
        let targets = list_live_instances(ctx.client, ctx.namespace).await?;
        Ok(targets
            .into_iter()
            .find(|target| target.instance.as_str() == instance))
    }
}

enum EnsurePod {
    Ready,
    NotFound,
    NotManaged,
    Failed(String),
}

enum StopBlock {
    NotFound,
    NotManaged,
    Failed(String),
}

enum StartResult {
    Ready(BootState),
    Failed(String),
}

async fn owner_before(ctx: &BackupCtx<'_>, instance: &str) -> InstanceOwner {
    match owner_of(ctx.client, ctx.namespace, instance).await {
        Ok(Some(owner)) => owner,
        Ok(None) => InstanceOwner::default(),
        Err(err) => {
            warn!(error = ?err, instance, "could not resolve the owner for a backup event");
            InstanceOwner::default()
        }
    }
}

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

fn manifest(
    kind: ArtifactKind,
    instance: &InstanceName,
    game: &GameId,
    guild: &GuildId,
    created_by: &str,
    tarball_key: &str,
    size_bytes: u64,
) -> BackupManifest {
    BackupManifest {
        schema: MANIFEST_SCHEMA,
        kind,
        instance: instance.clone(),
        game: game.clone(),
        guild: guild.clone(),
        created_by: created_by.to_owned(),
        created_at: Timestamp::now().to_string(),
        tarball_key: tarball_key.to_owned(),
        size_bytes,
    }
}

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

#[cfg(test)]
#[path = "tests/orchestrate.rs"]
mod tests;
