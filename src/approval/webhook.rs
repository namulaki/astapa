#![allow(dead_code)]
use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum WebhookEvent {
    MutationStaged {
        session_id: String,
        project_id: String,
        mutation_count: usize,
        message: Option<String>,
    },
    MutationApproved {
        session_id: String,
        project_id: String,
        applied_count: usize,
    },
    MutationRejected {
        session_id: String,
        project_id: String,
        reason: Option<String>,
    },
    MutationConflict {
        session_id: String,
        project_id: String,
        conflicts: Vec<super::conflict::Conflict>,
    },
    SessionExpired {
        session_id: String,
        project_id: String,
    },
}

#[derive(Debug, Error)]
pub enum WebhookError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("Webhook URL not configured")]
    NotConfigured,
}

#[derive(Debug, Clone)]
pub struct WebhookDispatcher {
    client: Option<reqwest::Client>,
    webhook_url: Option<String>,
    webhook_secret: Option<String>,
}

impl WebhookDispatcher {
    pub fn new(webhook_url: Option<String>, webhook_secret: Option<String>) -> Self {
        let client = webhook_url.as_ref().map(|_| {
            reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .expect("Failed to create HTTP client")
        });

        Self {
            client,
            webhook_url,
            webhook_secret,
        }
    }

    /// Disabled dispatcher (no webhooks)
    pub fn disabled() -> Self {
        Self {
            client: None,
            webhook_url: None,
            webhook_secret: None,
        }
    }

    /// Send webhook event (non-blocking, fire-and-forget)
    pub fn send(&self, event: WebhookEvent) {
        if let (Some(client), Some(url)) = (&self.client, &self.webhook_url) {
            let client = client.clone();
            let url = url.clone();
            let secret = self.webhook_secret.clone();

            tokio::spawn(async move {
                if let Err(e) = send_webhook(&client, &url, secret.as_deref(), &event).await {
                    tracing::warn!("Webhook delivery failed: {}", e);
                }
            });
        }
    }

    /// Send webhook event and wait for response
    pub async fn send_and_wait(&self, event: WebhookEvent) -> Result<(), WebhookError> {
        let client = self.client.as_ref().ok_or(WebhookError::NotConfigured)?;
        let url = self
            .webhook_url
            .as_ref()
            .ok_or(WebhookError::NotConfigured)?;

        send_webhook(client, url, self.webhook_secret.as_deref(), &event).await
    }
}

async fn send_webhook(
    client: &reqwest::Client,
    url: &str,
    secret: Option<&str>,
    event: &WebhookEvent,
) -> Result<(), WebhookError> {
    let payload = serde_json::to_string(event).expect("Failed to serialize webhook");

    let mut request = client
        .post(url)
        .header("Content-Type", "application/json")
        .body(payload.clone());

    // Add signature if secret is configured
    if let Some(secret) = secret {
        let signature = compute_signature(&payload, secret);
        request = request.header("X-Astapa-Signature", format!("sha256={}", signature));
    }

    let response = request.send().await?;

    if !response.status().is_success() {
        tracing::warn!(
            "Webhook returned non-success status: {} for {}",
            response.status(),
            event_type(event)
        );
    }

    Ok(())
}

fn compute_signature(payload: &str, secret: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    // Simple signature for now - in production use HMAC-SHA256
    let mut hasher = DefaultHasher::new();
    payload.hash(&mut hasher);
    secret.hash(&mut hasher);
    format!("{:x}", hasher.finish())
}

fn event_type(event: &WebhookEvent) -> &'static str {
    match event {
        WebhookEvent::MutationStaged { .. } => "mutation_staged",
        WebhookEvent::MutationApproved { .. } => "mutation_approved",
        WebhookEvent::MutationRejected { .. } => "mutation_rejected",
        WebhookEvent::MutationConflict { .. } => "mutation_conflict",
        WebhookEvent::SessionExpired { .. } => "session_expired",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_serialization() {
        let event = WebhookEvent::MutationApproved {
            session_id: "abc123".to_string(),
            project_id: "proj_1".to_string(),
            applied_count: 5,
        };

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("mutation_approved"));
        assert!(json.contains("abc123"));
    }
}
