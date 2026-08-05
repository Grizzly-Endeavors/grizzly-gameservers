//! Namespace capacity: whether the hardware can admit one more game pod at all.
//!
//! [`crate::limits`] bounds what one guild may hold; this bounds what the
//! namespace holds in total. They are different refusals and must read
//! differently — a guild cap is "your community is using its share", which
//! somebody in that guild can act on by stopping a server, while a namespace
//! ceiling is "there is no room left on the hardware", which they cannot.
//!
//! The authority is the namespace `ResourceQuota`, read live rather than
//! mirrored here: it is the object admission actually enforces, so a second copy
//! of its figures would be a second thing to keep in step. What this module adds
//! is *when* the check happens. Kubernetes enforces a quota at pod admission —
//! and for a game server the pod is created by Agones, minutes after the
//! friend's command already returned an address. An unpriced create therefore
//! leases a port and a PVC for a pod that will never be admitted, and the friend
//! waits out the readiness timeout only to be told the server is "still
//! starting". Pricing the admission before anything is leased turns that into an
//! immediate refusal that says what is actually wrong.

use crate::limits::{format_cores, human_size};

const MIB: i64 = 1024 * 1024;
const GIB: i64 = 1024 * MIB;

/// One quota dimension: what the namespace allows, and what it already uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Dimension {
    pub(crate) hard: i64,
    pub(crate) used: i64,
}

impl Dimension {
    /// What is left after `reserved` is held back, floored at zero — a quota
    /// lowered below current usage would otherwise report negative room.
    fn free(self, reserved: i64) -> i64 {
        self.hard
            .saturating_sub(self.used)
            .saturating_sub(reserved)
            .max(0)
    }
}

/// A namespace `ResourceQuota` reduced to the dimensions a game server consumes.
///
/// Every field is optional because a quota constrains only the keys it names,
/// and an unnamed dimension can't refuse anything. An entirely empty snapshot —
/// no quota object, or one the bot may not read — therefore admits everything,
/// which is the correct degradation: the cluster still enforces whatever it
/// enforces, and a bot that can't see the ceiling must not invent one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct NamespaceQuota {
    pub(crate) pvcs: Option<Dimension>,
    pub(crate) storage_bytes: Option<Dimension>,
    pub(crate) cpu_requests: Option<Dimension>,
    pub(crate) cpu_limits: Option<Dimension>,
    pub(crate) mem_requests: Option<Dimension>,
    pub(crate) mem_limits: Option<Dimension>,
}

/// What one admission adds to the namespace's usage: the pod's declared figures
/// plus, for a create, the PVC that comes with it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Admission {
    pub(crate) pvcs: i64,
    pub(crate) storage_bytes: i64,
    pub(crate) cpu_request_millicores: i64,
    pub(crate) cpu_limit_millicores: i64,
    pub(crate) mem_request_bytes: i64,
    pub(crate) mem_limit_bytes: i64,
}

impl Admission {
    /// Sum two contributions to one pod's footprint.
    pub(crate) fn plus(self, other: Self) -> Self {
        Self {
            pvcs: self.pvcs.saturating_add(other.pvcs),
            storage_bytes: self.storage_bytes.saturating_add(other.storage_bytes),
            cpu_request_millicores: self
                .cpu_request_millicores
                .saturating_add(other.cpu_request_millicores),
            cpu_limit_millicores: self
                .cpu_limit_millicores
                .saturating_add(other.cpu_limit_millicores),
            mem_request_bytes: self
                .mem_request_bytes
                .saturating_add(other.mem_request_bytes),
            mem_limit_bytes: self.mem_limit_bytes.saturating_add(other.mem_limit_bytes),
        }
    }
}

/// The Agones SDK sidecar, injected into every game pod by the Agones
/// controller. It never appears in a catalog template, but the quota prices the
/// whole pod — so leaving it out of the estimate is how a create passes the
/// check here and is still rejected at admission. Figures mirror
/// `cluster/agones/helmrelease.yaml`, which is where they are actually set.
pub(crate) const SDK_SIDECAR: Admission = Admission {
    pvcs: 0,
    storage_bytes: 0,
    cpu_request_millicores: 30,
    cpu_limit_millicores: 200,
    mem_request_bytes: 64 * MIB,
    mem_limit_bytes: 128 * MIB,
};

/// Headroom withheld from every game admission so the bot's own next pod stays
/// schedulable.
///
/// The bot runs in the namespace it hands out, its Deployment rolls with the
/// default surge, and quota is first-come-first-served — so without this, game
/// pods (created continuously) can consume the room the replacement bot pod
/// (created once) needs, and the rollout stalls while CI and Flux both report
/// success. Reserving one bot pod's worth means the last slice of the namespace
/// is never handed to a game server. Figures mirror the chart's `bot.resources`
/// in `deploy/values.yaml`.
const BOT_ROLLOUT_RESERVE: Admission = Admission {
    pvcs: 0,
    storage_bytes: 0,
    cpu_request_millicores: 250,
    cpu_limit_millicores: 2000,
    mem_request_bytes: 512 * MIB,
    mem_limit_bytes: 2 * GIB,
};

/// Which side of a quota dimension refused — the request figures the scheduler
/// reserves, or the limit figures that bound the blast radius. Invisible to a
/// friend, who is told the same thing either way, but the log and the event
/// record it: which of the two binds first is exactly what an operator needs to
/// know when deciding what to raise.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum QuotaBasis {
    Requests,
    Limits,
}

impl QuotaBasis {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Requests => "requests",
            Self::Limits => "limits",
        }
    }
}

/// Why the namespace can't take another server, carrying the figures so every
/// surface can name them rather than saying "full".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CapacityRefusal {
    /// One PVC per server, so the PVC ceiling *is* the server ceiling.
    Servers {
        current: i64,
        ceiling: i64,
    },
    Storage {
        free_bytes: i64,
        adding_bytes: i64,
    },
    Cpu {
        basis: QuotaBasis,
        free_millicores: i64,
        adding_millicores: i64,
    },
    Memory {
        basis: QuotaBasis,
        free_bytes: i64,
        adding_bytes: i64,
    },
    /// Admission refused the pod, but a re-read of the quota no longer says
    /// which dimension bound — usage moved in between, which is the same race
    /// that let the create past the check in the first place. Never produced by
    /// [`check_admission`]; only by a caller reacting to a live rejection.
    Unspecified,
}

impl CapacityRefusal {
    /// A short operator-facing summary for logs and the event log. Not
    /// friend-facing — the surfaces render their own wording from the figures.
    pub(crate) fn summary(self) -> String {
        match self {
            Self::Servers { current, ceiling } => {
                format!("server count {current}/{ceiling}")
            }
            Self::Storage {
                free_bytes,
                adding_bytes,
            } => format!(
                "storage needs {}, {} free",
                human_size(adding_bytes.unsigned_abs()),
                human_size(free_bytes.unsigned_abs())
            ),
            Self::Cpu {
                basis,
                free_millicores,
                adding_millicores,
            } => format!(
                "cpu {} needs {}, {} free",
                basis.as_str(),
                format_cores(adding_millicores),
                format_cores(free_millicores)
            ),
            Self::Memory {
                basis,
                free_bytes,
                adding_bytes,
            } => format!(
                "memory {} needs {}, {} free",
                basis.as_str(),
                human_size(adding_bytes.unsigned_abs()),
                human_size(free_bytes.unsigned_abs())
            ),
            Self::Unspecified => "quota exhausted, dimension unresolved".to_owned(),
        }
    }
}

/// Whether the namespace can admit `adding`, with the bot's rollout headroom
/// held back.
///
/// Dimensions are checked cheapest-to-explain first, and the first one that
/// binds is the one reported: a friend who is told the cluster is out of memory
/// can act on that, while a list of four exhausted dimensions is noise.
///
/// # Errors
///
/// Returns the first dimension that cannot take the admission.
pub(crate) fn check_admission(
    quota: &NamespaceQuota,
    adding: Admission,
) -> Result<(), CapacityRefusal> {
    if let Some(pvcs) = quota.pvcs
        && adding.pvcs > pvcs.free(BOT_ROLLOUT_RESERVE.pvcs)
    {
        return Err(CapacityRefusal::Servers {
            current: pvcs.used,
            ceiling: pvcs.hard,
        });
    }
    if let Some(free) = shortfall(
        quota.storage_bytes,
        adding.storage_bytes,
        BOT_ROLLOUT_RESERVE.storage_bytes,
    ) {
        return Err(CapacityRefusal::Storage {
            free_bytes: free,
            adding_bytes: adding.storage_bytes,
        });
    }

    for (basis, dimension, adding_millicores, reserve) in [
        (
            QuotaBasis::Requests,
            quota.cpu_requests,
            adding.cpu_request_millicores,
            BOT_ROLLOUT_RESERVE.cpu_request_millicores,
        ),
        (
            QuotaBasis::Limits,
            quota.cpu_limits,
            adding.cpu_limit_millicores,
            BOT_ROLLOUT_RESERVE.cpu_limit_millicores,
        ),
    ] {
        if let Some(free) = shortfall(dimension, adding_millicores, reserve) {
            return Err(CapacityRefusal::Cpu {
                basis,
                free_millicores: free,
                adding_millicores,
            });
        }
    }

    for (basis, dimension, adding_bytes, reserve) in [
        (
            QuotaBasis::Requests,
            quota.mem_requests,
            adding.mem_request_bytes,
            BOT_ROLLOUT_RESERVE.mem_request_bytes,
        ),
        (
            QuotaBasis::Limits,
            quota.mem_limits,
            adding.mem_limit_bytes,
            BOT_ROLLOUT_RESERVE.mem_limit_bytes,
        ),
    ] {
        if let Some(free) = shortfall(dimension, adding_bytes, reserve) {
            return Err(CapacityRefusal::Memory {
                basis,
                free_bytes: free,
                adding_bytes,
            });
        }
    }
    Ok(())
}

/// The room left in `dimension` when it can't take `adding`, or `None` when it
/// can (or doesn't constrain anything).
fn shortfall(dimension: Option<Dimension>, adding: i64, reserve: i64) -> Option<i64> {
    let free = dimension?.free(reserve);
    (adding > free).then_some(free)
}

#[cfg(test)]
#[path = "tests/capacity.rs"]
mod tests;
