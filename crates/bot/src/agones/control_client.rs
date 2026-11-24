use reqwest::{Client, RequestBuilder};

#[derive(Clone)]
pub(crate) struct ControlClient {
    inner: Client,
    credential: Option<String>,
}

impl ControlClient {
    pub(crate) fn new(inner: Client, token: Option<&str>) -> Self {
        Self {
            inner,
            credential: token
                .map(str::trim)
                .filter(|token| !token.is_empty())
                .map(|token| format!("Bearer {token}")),
        }
    }

    pub(crate) fn is_authenticated(&self) -> bool {
        self.credential.is_some()
    }

    pub(crate) fn get(&self, url: &str) -> RequestBuilder {
        self.authorize(self.inner.get(url))
    }

    pub(crate) fn post(&self, url: &str) -> RequestBuilder {
        self.authorize(self.inner.post(url))
    }

    fn authorize(&self, request: RequestBuilder) -> RequestBuilder {
        match self.credential.as_deref() {
            Some(credential) => request.header(reqwest::header::AUTHORIZATION, credential),
            None => request,
        }
    }
}

#[cfg(test)]
#[path = "tests/control_client.rs"]
mod tests;
