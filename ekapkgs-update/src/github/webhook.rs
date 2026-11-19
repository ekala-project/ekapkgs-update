//! GitHub webhook deliveries: signature verification and the events watch
//! acts on. Pure functions; the HTTP endpoint is in
//! `commands/watch/webhook.rs`.

use openssl::hash::MessageDigest;
use openssl::pkey::PKey;
use openssl::sign::Signer;
use serde::Deserialize;

/// Whether `signature` (the `X-Hub-Signature-256` header,
/// `sha256=<hex HMAC-SHA256 of the body>`) matches `body` under `secret`.
/// Compared in constant time.
pub fn verify_signature(secret: &[u8], body: &[u8], signature: Option<&str>) -> bool {
    let Some(hex) = signature.and_then(|s| s.strip_prefix("sha256=")) else {
        return false;
    };
    let Some(given) = decode_hex(hex) else {
        return false;
    };
    let Ok(expected) = hmac_sha256(secret, body) else {
        return false;
    };
    given.len() == expected.len() && openssl::memcmp::eq(&given, &expected)
}

/// The `X-Hub-Signature-256` value GitHub sends for `body` under `secret`.
#[cfg(test)]
pub fn signature(secret: &[u8], body: &[u8]) -> anyhow::Result<String> {
    let mac = hmac_sha256(secret, body)?;
    Ok(format!(
        "sha256={}",
        mac.iter().map(|b| format!("{b:02x}")).collect::<String>()
    ))
}

fn hmac_sha256(secret: &[u8], body: &[u8]) -> Result<Vec<u8>, openssl::error::ErrorStack> {
    let key = PKey::hmac(secret)?;
    let mut signer = Signer::new(MessageDigest::sha256(), &key)?;
    signer.update(body)?;
    signer.sign_to_vec()
}

fn decode_hex(hex: &str) -> Option<Vec<u8>> {
    if hex.len() % 2 != 0 {
        return None;
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok())
        .collect()
}

/// A delivery, reduced to what watch acts on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebhookEvent {
    /// Sent when the webhook is created or tested.
    Ping,
    /// Commits pushed to a branch of `repository` (`owner/name`).
    Push { repository: String, branch: String },
    /// A pull request into `repository` was closed, merged or not.
    PullRequestClosed {
        repository: String,
        number: i64,
        merged: bool,
    },
    /// Anything else; carries a short description for the delivery log.
    Ignored(String),
}

impl WebhookEvent {
    /// The repository (`owner/name`) the event is about.
    pub fn repository(&self) -> Option<&str> {
        match self {
            Self::Push { repository, .. } | Self::PullRequestClosed { repository, .. } => {
                Some(repository)
            },
            Self::Ping | Self::Ignored(_) => None,
        }
    }
}

#[derive(Deserialize)]
struct Repository {
    full_name: String,
}

#[derive(Deserialize)]
struct PushPayload {
    #[serde(rename = "ref")]
    git_ref: String,
    #[serde(default)]
    deleted: bool,
    repository: Repository,
}

#[derive(Deserialize)]
struct PullRequestPayload {
    action: String,
    number: i64,
    pull_request: PullRequest,
    repository: Repository,
}

#[derive(Deserialize)]
struct PullRequest {
    #[serde(default)]
    merged: bool,
}

/// Parse a delivery of type `event` (the `X-GitHub-Event` header).
pub fn parse_event(event: &str, body: &[u8]) -> anyhow::Result<WebhookEvent> {
    Ok(match event {
        "ping" => WebhookEvent::Ping,
        "push" => {
            let push: PushPayload = serde_json::from_slice(body)?;
            match push.git_ref.strip_prefix("refs/heads/") {
                Some(branch) if !push.deleted => WebhookEvent::Push {
                    repository: push.repository.full_name,
                    branch: branch.to_owned(),
                },
                Some(_) => WebhookEvent::Ignored("branch deleted".to_owned()),
                None => WebhookEvent::Ignored(format!("push to {}", push.git_ref)),
            }
        },
        "pull_request" => {
            let pr: PullRequestPayload = serde_json::from_slice(body)?;
            if pr.action == "closed" {
                WebhookEvent::PullRequestClosed {
                    repository: pr.repository.full_name,
                    number: pr.number,
                    merged: pr.pull_request.merged,
                }
            } else {
                WebhookEvent::Ignored(format!("pull_request {}", pr.action))
            }
        },
        other => WebhookEvent::Ignored(format!("{other} event")),
    })
}

/// The top-level `action` of a payload, if it has one (for the delivery log).
pub fn payload_action(body: &[u8]) -> Option<String> {
    #[derive(Deserialize)]
    struct Action {
        action: Option<String>,
    }
    serde_json::from_slice::<Action>(body).ok()?.action
}

#[cfg(test)]
mod tests {
    use super::*;

    /// GitHub's documented test vector.
    #[test]
    fn signature_matches_github_example() {
        let secret = b"It's a Secret to Everybody";
        let body = b"Hello, World!";
        let good = "sha256=757107ea0eb2509fc211221cce984b8a37570b6d7586c22c46f4379c8b043e17";
        assert!(verify_signature(secret, body, Some(good)));
        assert!(verify_signature(
            secret,
            body,
            Some(&good.to_uppercase().replace("SHA256=", "sha256="))
        ));

        for bad in [
            None,
            Some(""),
            Some("sha256="),
            Some("757107ea0eb2509fc211221cce984b8a37570b6d7586c22c46f4379c8b043e17"),
            Some("sha1=757107ea0eb2509fc211221cce984b8a37570b6d7586c22c46f4379c8b043e17"),
            Some("sha256=757107ea0eb2509fc211221cce984b8a37570b6d7586c22c46f4379c8b043e18"),
            Some("sha256=757107ea"),
            Some("sha256=zz7107ea0eb2509fc211221cce984b8a37570b6d7586c22c46f4379c8b043e17"),
        ] {
            assert!(!verify_signature(secret, body, bad), "{bad:?}");
        }
        assert_eq!(signature(secret, body).unwrap(), good);
        assert!(!verify_signature(b"other secret", body, Some(good)));
        assert!(!verify_signature(secret, b"Hello, World?", Some(good)));
    }

    #[test]
    fn events() {
        let repo = r#""repository":{"full_name":"ekala-project/corepkgs"}"#;
        let push = |git_ref: &str, deleted: bool| {
            format!(r#"{{"ref":"{git_ref}","deleted":{deleted},"after":"abc",{repo}}}"#)
        };
        let pr = |action: &str, merged: bool| {
            format!(
                r#"{{"action":"{action}","number":302,"pull_request":{{"merged":{merged}}},{repo}}}"#
            )
        };
        let core = "ekala-project/corepkgs".to_owned();
        let cases = [
            (
                "ping",
                r#"{"zen":"Keep it logically awesome."}"#.to_owned(),
                WebhookEvent::Ping,
            ),
            (
                "push",
                push("refs/heads/master", false),
                WebhookEvent::Push {
                    repository: core.clone(),
                    branch: "master".to_owned(),
                },
            ),
            (
                "push",
                push("refs/heads/update/grpc/1.85.3", true),
                WebhookEvent::Ignored("branch deleted".to_owned()),
            ),
            (
                "push",
                push("refs/tags/v1.0", false),
                WebhookEvent::Ignored("push to refs/tags/v1.0".to_owned()),
            ),
            (
                "pull_request",
                pr("closed", true),
                WebhookEvent::PullRequestClosed {
                    repository: core.clone(),
                    number: 302,
                    merged: true,
                },
            ),
            (
                "pull_request",
                pr("closed", false),
                WebhookEvent::PullRequestClosed {
                    repository: core.clone(),
                    number: 302,
                    merged: false,
                },
            ),
            (
                "pull_request",
                pr("opened", false),
                WebhookEvent::Ignored("pull_request opened".to_owned()),
            ),
            (
                "issues",
                r#"{"action":"opened"}"#.to_owned(),
                WebhookEvent::Ignored("issues event".to_owned()),
            ),
        ];
        for (event, body, expected) in cases {
            assert_eq!(
                parse_event(event, body.as_bytes()).unwrap(),
                expected,
                "{event} {body}"
            );
        }
        assert!(parse_event("push", b"not json").is_err());
        assert!(parse_event("pull_request", b"{}").is_err());
    }

    #[test]
    fn actions() {
        assert_eq!(
            payload_action(br#"{"action":"closed"}"#).as_deref(),
            Some("closed")
        );
        assert_eq!(payload_action(br#"{"ref":"refs/heads/master"}"#), None);
        assert_eq!(payload_action(b"nope"), None);
    }
}
