//! The two calls of ferret's ingest API this needs: put and delete a document.

use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use ureq::Agent;

use crate::render::Document;

pub struct Client {
    agent: Agent,
    url: String,
    auth: String,
}

#[derive(Debug, PartialEq)]
pub enum Outcome {
    Created,
    Updated,
    Unchanged,
    /// The user excluded this document in ferret; it stays out.
    Excluded,
}

/// A rejected credential stops the run instead of failing every document.
#[derive(Debug)]
pub struct Unauthorized(pub u16, pub String);

impl std::fmt::Display for Unauthorized {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ferret rejected the API key ({}): {}", self.0, self.1)
    }
}

impl std::error::Error for Unauthorized {}

const ATTEMPTS: u32 = 4;

impl Client {
    pub fn new(url: &str, key: &str) -> Client {
        let agent: Agent = Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(60)))
            .http_status_as_error(false)
            .build()
            .into();
        Client {
            agent,
            url: url.to_owned(),
            auth: format!("Bearer {key}"),
        }
    }

    pub fn put(&self, doc: &Document) -> Result<Outcome> {
        let url = format!("{}/api/documents", self.url);
        let (status, body) = self.retrying(|| {
            self.agent
                .put(&url)
                .header("Authorization", &self.auth)
                .send_json(doc)
        })?;
        if status != 200 {
            bail!("put {}: HTTP {status}: {body}", doc.external_id);
        }
        let outcome: serde_json::Value = serde_json::from_str(&body)?;
        match outcome.get("outcome").and_then(|o| o.as_str()) {
            Some("created") => Ok(Outcome::Created),
            Some("updated") => Ok(Outcome::Updated),
            Some("unchanged") => Ok(Outcome::Unchanged),
            Some("excluded") => Ok(Outcome::Excluded),
            _ => Err(anyhow!("put {}: unexpected answer {body}", doc.external_id)),
        }
    }

    /// Deleting a document that isn't there counts as done.
    pub fn delete(&self, source: &str, external_id: &str) -> Result<()> {
        let url = format!("{}/api/documents", self.url);
        let (status, body) = self.retrying(|| {
            self.agent
                .delete(&url)
                .header("Authorization", &self.auth)
                .query("source", source)
                .query("external_id", external_id)
                .call()
        })?;
        match status {
            200 | 204 | 404 => Ok(()),
            _ => bail!("delete {external_id}: HTTP {status}: {body}"),
        }
    }

    /// Retries transport errors, 429 and 5xx with backoff.
    fn retrying(
        &self,
        send: impl Fn() -> Result<ureq::http::Response<ureq::Body>, ureq::Error>,
    ) -> Result<(u16, String)> {
        let mut delay = Duration::from_millis(500);
        for attempt in 1..=ATTEMPTS {
            let last = attempt == ATTEMPTS;
            match send() {
                Ok(mut response) => {
                    let status = response.status().as_u16();
                    let body = response.body_mut().read_to_string().unwrap_or_default();
                    if status == 401 || status == 403 {
                        return Err(Unauthorized(status, body).into());
                    }
                    if (status == 429 || status >= 500) && !last {
                        std::thread::sleep(delay);
                        delay *= 2;
                        continue;
                    }
                    return Ok((status, body));
                }
                Err(error) if !last => {
                    eprintln!("retrying after {error}");
                    std::thread::sleep(delay);
                    delay *= 2;
                }
                Err(error) => return Err(error.into()),
            }
        }
        unreachable!("the last attempt returns")
    }
}
