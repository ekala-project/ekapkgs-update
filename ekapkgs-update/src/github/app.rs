//! GitHub App authentication.
//!
//! The app signs a short-lived JWT (RS256) with its private key, and trades
//! it for an *installation access token* per installation
//! (`POST /app/installations/{id}/access_tokens`). Installation tokens last
//! an hour; they are cached and renewed [`TOKEN_RENEW_BEFORE`] ahead of
//! expiry. Requests and pushes made with them are attributed to the app's bot
//! account (`<slug>[bot]`), not to a person.

use std::collections::HashMap;

use anyhow::Context;
use chrono::{DateTime, Duration, Utc};
use openssl::hash::MessageDigest;
use openssl::pkey::{PKey, Private};
use openssl::sign::Signer;
use reqwest::{Client, Method};
use serde::Deserialize;
use tokio::sync::{Mutex, OnceCell};
use tracing::{debug, info};

use super::github_request_builder;

const API: &str = "https://api.github.com";
/// JWT `iat` is backdated this far to tolerate clock drift (GitHub's advice).
const JWT_BACKDATE_SECS: i64 = 60;
/// JWT lifetime; GitHub allows at most 10 minutes.
const JWT_LIFETIME_SECS: i64 = 9 * 60;
/// Installation tokens are renewed when they expire within this window.
const TOKEN_RENEW_BEFORE: Duration = Duration::minutes(5);

/// A GitHub App: its client ID and private key, plus cached installation
/// lookups and tokens.
pub struct GithubApp {
    client_id: String,
    key: PKey<Private>,
    client: Client,
    /// `owner/repo` (lowercase) → installation id.
    installations: Mutex<HashMap<String, u64>>,
    tokens: Mutex<HashMap<u64, CachedToken>>,
    identity: OnceCell<BotIdentity>,
}

impl std::fmt::Debug for GithubApp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GithubApp")
            .field("client_id", &self.client_id)
            .finish_non_exhaustive()
    }
}

#[derive(Clone)]
struct CachedToken {
    token: String,
    expires_at: DateTime<Utc>,
}

/// The git identity of an app's bot account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BotIdentity {
    /// `<slug>[bot]`
    pub name: String,
    /// `<user id>+<slug>[bot]@users.noreply.github.com`, which GitHub links
    /// to the bot account.
    pub email: String,
}

impl BotIdentity {
    fn new(slug: &str, user_id: u64) -> Self {
        let name = format!("{slug}[bot]");
        let email = format!("{user_id}+{name}@users.noreply.github.com");
        Self { name, email }
    }
}

impl GithubApp {
    /// An app from its client ID (or app ID) and PEM private key (PKCS#1 as
    /// downloaded from GitHub, or PKCS#8).
    pub fn new(client_id: &str, private_key_pem: &[u8]) -> anyhow::Result<Self> {
        let key = PKey::private_key_from_pem(private_key_pem)
            .context("parse the GitHub App private key (PEM)")?;
        Ok(Self {
            client_id: client_id.to_owned(),
            key,
            client: Client::new(),
            installations: Mutex::new(HashMap::new()),
            tokens: Mutex::new(HashMap::new()),
            identity: OnceCell::new(),
        })
    }

    pub fn client_id(&self) -> &str {
        &self.client_id
    }

    /// The app JWT, valid for [`JWT_LIFETIME_SECS`] from `now` (Unix time).
    pub fn jwt(&self, now: i64) -> anyhow::Result<String> {
        let header = base64url(br#"{"alg":"RS256","typ":"JWT"}"#);
        let claims = serde_json::json!({
            "iat": now - JWT_BACKDATE_SECS,
            "exp": now + JWT_LIFETIME_SECS,
            "iss": self.client_id,
        });
        let claims = base64url(claims.to_string().as_bytes());
        let signing_input = format!("{header}.{claims}");
        let mut signer =
            Signer::new(MessageDigest::sha256(), &self.key).context("create JWT signer")?;
        signer
            .update(signing_input.as_bytes())
            .context("sign JWT")?;
        let signature = signer.sign_to_vec().context("sign JWT")?;
        Ok(format!("{signing_input}.{}", base64url(&signature)))
    }

    /// An installation token that can act on `owner/repo` (where the app
    /// must be installed).
    pub async fn installation_token(&self, owner: &str, repo: &str) -> anyhow::Result<String> {
        let id = self.installation_id(owner, repo).await?;
        self.token_for_installation(id).await
    }

    /// An installation token of any installation, for reading public data.
    pub async fn any_installation_token(&self) -> anyhow::Result<String> {
        let cached = self.tokens.lock().await.keys().next().copied();
        let id = match cached {
            Some(id) => id,
            None => {
                #[derive(Deserialize)]
                struct Installation {
                    id: u64,
                }
                let installations: Vec<Installation> = self
                    .app_get(&format!("{API}/app/installations?per_page=1"))
                    .await?;
                installations
                    .first()
                    .map(|i| i.id)
                    .context("the GitHub App is not installed anywhere")?
            },
        };
        self.token_for_installation(id).await
    }

    /// The git identity of the app's bot account (looked up once).
    pub async fn bot_identity(&self) -> anyhow::Result<BotIdentity> {
        self.identity
            .get_or_try_init(|| async {
                #[derive(Deserialize)]
                struct App {
                    slug: String,
                }
                #[derive(Deserialize)]
                struct User {
                    id: u64,
                }
                let app: App = self.app_get(&format!("{API}/app")).await?;
                let url = format!("{API}/users/{}%5Bbot%5D", app.slug);
                let token = self.any_installation_token().await.ok();
                let user: User = get_json(&self.client, &url, token.as_deref()).await?;
                let identity = BotIdentity::new(&app.slug, user.id);
                info!(
                    "GitHub App commits as {} <{}>",
                    identity.name, identity.email
                );
                Ok(identity)
            })
            .await
            .cloned()
    }

    async fn installation_id(&self, owner: &str, repo: &str) -> anyhow::Result<u64> {
        let key = format!("{owner}/{repo}").to_lowercase();
        if let Some(id) = self.installations.lock().await.get(&key) {
            return Ok(*id);
        }
        #[derive(Deserialize)]
        struct Installation {
            id: u64,
        }
        let installation: Installation = self
            .app_get(&format!("{API}/repos/{owner}/{repo}/installation"))
            .await
            .with_context(|| format!("is the GitHub App installed on {owner}/{repo}?"))?;
        self.installations.lock().await.insert(key, installation.id);
        Ok(installation.id)
    }

    async fn token_for_installation(&self, id: u64) -> anyhow::Result<String> {
        let mut tokens = self.tokens.lock().await;
        if let Some(cached) = tokens.get(&id) {
            if cached.expires_at - Utc::now() > TOKEN_RENEW_BEFORE {
                return Ok(cached.token.clone());
            }
        }
        #[derive(Deserialize)]
        struct AccessToken {
            token: String,
            expires_at: DateTime<Utc>,
        }
        let url = format!("{API}/app/installations/{id}/access_tokens");
        let jwt = self.jwt(Utc::now().timestamp())?;
        let response = github_request_builder(&self.client, Method::POST, &url, Some(&jwt))
            .send()
            .await
            .with_context(|| format!("POST {url}"))?;
        let access: AccessToken = json_or_error(response, &url).await?;
        debug!(
            "Minted a GitHub App installation token for installation {} (expires {})",
            id, access.expires_at
        );
        tokens.insert(
            id,
            CachedToken {
                token: access.token.clone(),
                expires_at: access.expires_at,
            },
        );
        Ok(access.token)
    }

    /// GET an `/app` endpoint authenticated as the app (JWT).
    async fn app_get<T: serde::de::DeserializeOwned>(&self, url: &str) -> anyhow::Result<T> {
        let jwt = self.jwt(Utc::now().timestamp())?;
        get_json(&self.client, url, Some(&jwt)).await
    }
}

async fn get_json<T: serde::de::DeserializeOwned>(
    client: &Client,
    url: &str,
    token: Option<&str>,
) -> anyhow::Result<T> {
    let response = github_request_builder(client, Method::GET, url, token)
        .send()
        .await
        .with_context(|| format!("GET {url}"))?;
    json_or_error(response, url).await
}

async fn json_or_error<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
    url: &str,
) -> anyhow::Result<T> {
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("{url} returned {status}: {}", body.trim());
    }
    response
        .json()
        .await
        .with_context(|| format!("parse response from {url}"))
}

/// Unpadded base64url (RFC 4648 §5), as JWTs use.
fn base64url(data: &[u8]) -> String {
    openssl::base64::encode_block(data)
        .trim_end_matches('=')
        .replace('+', "-")
        .replace('/', "_")
}

#[cfg(test)]
mod tests {
    use openssl::rsa::Rsa;
    use openssl::sign::Verifier;

    use super::*;

    fn decode(segment: &str) -> Vec<u8> {
        let mut b64 = segment.replace('-', "+").replace('_', "/");
        while b64.len() % 4 != 0 {
            b64.push('=');
        }
        openssl::base64::decode_block(&b64).unwrap()
    }

    #[test]
    fn base64url_is_unpadded_and_url_safe() {
        assert_eq!(base64url(b""), "");
        assert_eq!(base64url(b"f"), "Zg");
        assert_eq!(base64url(b"fo"), "Zm8");
        assert_eq!(base64url(b"foo"), "Zm9v");
        assert_eq!(base64url(&[0xfb, 0xff]), "-_8");
    }

    #[test]
    fn jwt_is_rs256_signed_with_github_claims() {
        let rsa = Rsa::generate(2048).unwrap();
        let pem = rsa.private_key_to_pem().unwrap();
        let public =
            PKey::from_rsa(Rsa::public_key_from_pem(&rsa.public_key_to_pem().unwrap()).unwrap())
                .unwrap();
        let app = GithubApp::new("Iv23liExample", &pem).unwrap();

        let jwt = app.jwt(1_700_000_000).unwrap();
        let parts: Vec<&str> = jwt.split('.').collect();
        assert_eq!(parts.len(), 3);

        let header: serde_json::Value = serde_json::from_slice(&decode(parts[0])).unwrap();
        assert_eq!(header["alg"], "RS256");
        let claims: serde_json::Value = serde_json::from_slice(&decode(parts[1])).unwrap();
        assert_eq!(claims["iss"], "Iv23liExample");
        assert_eq!(claims["iat"], 1_700_000_000 - 60);
        assert_eq!(claims["exp"], 1_700_000_000 + 9 * 60);

        let mut verifier = Verifier::new(MessageDigest::sha256(), &public).unwrap();
        verifier
            .update(format!("{}.{}", parts[0], parts[1]).as_bytes())
            .unwrap();
        assert!(verifier.verify(&decode(parts[2])).unwrap());
    }

    #[test]
    fn rejects_a_bad_key() {
        assert!(GithubApp::new("id", b"not a key").is_err());
    }

    #[test]
    fn bot_identity_format() {
        assert_eq!(
            BotIdentity::new("ekapkgs-update", 41898282),
            BotIdentity {
                name: "ekapkgs-update[bot]".to_owned(),
                email: "41898282+ekapkgs-update[bot]@users.noreply.github.com".to_owned(),
            }
        );
    }
}
