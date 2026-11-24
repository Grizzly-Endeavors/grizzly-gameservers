use std::collections::BTreeMap;

use anyhow::Result;

#[derive(Default, serde::Deserialize)]
pub(crate) struct AnnotatedTemplate {
    #[serde(default)]
    metadata: TemplateMetadata,
    #[serde(default)]
    spec: TemplateSpec,
}

impl AnnotatedTemplate {
    pub(crate) fn parse(gameserver_yaml: &str) -> Result<Self> {
        Ok(serde_saphyr::from_str(gameserver_yaml)?)
    }

    pub(crate) fn annotations(&self) -> Option<&BTreeMap<String, String>> {
        self.metadata.annotations.as_ref()
    }

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
    value: Option<String>,
}

#[cfg(test)]
#[path = "tests/template.rs"]
mod tests;
