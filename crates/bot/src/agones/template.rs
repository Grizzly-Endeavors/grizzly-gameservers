//! The slice of a `GameServer` template the catalog reads at load time.
//!
//! Two per-game facts are declared on the template rather than in the image —
//! where a game's mods come from ([`super::mod_registry`]) and which settings an
//! instance may override ([`super::env_overlay`]) — because both are policy that
//! should ship through catalog review. Both read the same two places: the
//! metadata annotations and the game container's environment. That shared read
//! lives here so neither module owns a parse the other borrows.
//!
//! Deliberately partial: only the fields those lookups need are declared, so an
//! unrelated manifest edit cannot perturb the parse.

use std::collections::BTreeMap;

use anyhow::Result;

/// A parsed template, reduced to the annotations and container environment.
#[derive(Default, serde::Deserialize)]
pub(crate) struct AnnotatedTemplate {
    #[serde(default)]
    metadata: TemplateMetadata,
    #[serde(default)]
    spec: TemplateSpec,
}

impl AnnotatedTemplate {
    /// Parse a `GameServer` template.
    ///
    /// # Errors
    ///
    /// Returns an error if the template is not parseable YAML.
    pub(crate) fn parse(gameserver_yaml: &str) -> Result<Self> {
        Ok(serde_saphyr::from_str(gameserver_yaml)?)
    }

    /// This template's metadata annotations.
    pub(crate) fn annotations(&self) -> Option<&BTreeMap<String, String>> {
        self.metadata.annotations.as_ref()
    }

    /// The literal value of `name` in the pod's container environment, trimmed,
    /// with an empty value reported as absent — an env var set to `""` in a
    /// template means "declared but off", which every caller here treats the
    /// same as undeclared.
    ///
    /// A `valueFrom` variable has no literal value and is never returned: those
    /// name a secret, and none of the per-game facts read through here are one.
    pub(crate) fn env_value(&self, name: &str) -> Option<&str> {
        self.spec
            .template
            .spec
            .containers
            .iter()
            .flat_map(|container| &container.env)
            .find(|var| var.name == name)
            .and_then(|var| var.value.as_deref())
            .map(str::trim)
            .filter(|value| !value.is_empty())
    }
}

#[derive(Default, serde::Deserialize)]
struct TemplateMetadata {
    annotations: Option<BTreeMap<String, String>>,
}

#[derive(Default, serde::Deserialize)]
struct TemplateSpec {
    #[serde(default)]
    template: PodTemplate,
}

#[derive(Default, serde::Deserialize)]
struct PodTemplate {
    #[serde(default)]
    spec: PodSpec,
}

#[derive(Default, serde::Deserialize)]
struct PodSpec {
    #[serde(default)]
    containers: Vec<Container>,
}

#[derive(serde::Deserialize)]
struct Container {
    #[serde(default)]
    env: Vec<EnvVar>,
}

#[derive(serde::Deserialize)]
struct EnvVar {
    name: String,
    /// Absent for a `valueFrom` variable, which names a secret rather than a path.
    value: Option<String>,
}

#[cfg(test)]
#[path = "tests/template.rs"]
mod tests;
