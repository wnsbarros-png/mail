//! Incremental Google/Microsoft consent and independent Dropbox PKCE.
//! No tokens, token responses or preauthenticated session URLs are logged.
use super::*;
use crate::{
    accounts::credentials::{self, Slot},
    models::Provider,
};
use base64::Engine;
use serde_json::Value;
use std::time::Duration;
pub const DRIVE_SCOPE: &str = "https://www.googleapis.com/auth/drive.file";
pub const ONEDRIVE_SCOPE: &str = "https://graph.microsoft.com/Files.ReadWrite";
const DROPBOX_TOKEN: &str = "https://api.dropboxapi.com/oauth2/token";
#[derive(Serialize, Deserialize)]
struct DropboxBundle {
    app_key: String,
    refresh: String,
    access: String,
    expires_at: i64,
}
fn random() -> String {
    format!(
        "{:032x}{:032x}",
        rand::random::<u128>(),
        rand::random::<u128>()
    )
}
fn pkce(verifier: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}
async fn token(form: &[(&str, String)]) -> Result<Value> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|_| invalid("Could not initialize OAuth transport"))?;
    let response = client
        .post(DROPBOX_TOKEN)
        .form(form)
        .send()
        .await
        .map_err(|_| invalid("Dropbox OAuth connection failed"))?;
    if !response.status().is_success() {
        return Err(invalid(
            "Dropbox OAuth denied; reconnect in cloud sign-in settings",
        ));
    }
    let bytes = crate::http_body::bytes(response, 64 * 1024, "cloud OAuth")
        .await
        .map_err(|_| invalid("Could not read Dropbox token response"))?;
    let value: Value =
        serde_json::from_slice(&bytes).map_err(|_| invalid("Invalid Dropbox token response"))?;
    if value.get("access_token").and_then(Value::as_str).is_none() {
        return Err(invalid("Dropbox did not return an access token"));
    }
    Ok(value)
}
impl Core {
    /// User explicitly opens cloud sign-in settings. This never uploads a file.
    pub async fn connect_cloud(
        &self,
        provider: CloudProvider,
        account_id: i64,
        open_url: impl FnOnce(String) -> std::result::Result<(), String> + Send,
    ) -> Result<()> {
        credentials::check_available_async(self.credentials.clone()).await?;
        match provider {
            CloudProvider::OneDrive => {
                self.graph_token_with_consent(account_id,ONEDRIVE_SCOPE,open_url).await.map_err(|_|invalid("Could not authorize OneDrive. Check the Microsoft account and Files.ReadWrite consent."))?;
            }
            CloudProvider::GoogleDrive => {
                let account = self
                    .db
                    .read(move |conn| crate::db::repo::accounts::get(conn, account_id))
                    .await?
                    .ok_or_else(|| invalid("Select a Gmail account for Google Drive"))?;
                if account.provider != Provider::Gmail {
                    return Err(invalid("Select a Gmail account for Google Drive"));
                }
                let outcome = crate::oauth::flow::authorize_with_broker(
                    Provider::Gmail,
                    &[DRIVE_SCOPE],
                    Some(&account.email),
                    self.oauth_redirects.clone(),
                    open_url,
                )
                .await
                .map_err(|_| invalid("Google Drive consent failed"))?;
                self.store_sign_in(&account, outcome).await?;
            }
            CloudProvider::Dropbox => {
                let key = self.cloud_settings().dropbox_app_key;
                if key.is_empty() {
                    return Err(invalid(
                        "Add a Dropbox app key in cloud sign-in settings first",
                    ));
                }
                // Dropbox requires this exact URI in the App Console.
                let listener = crate::oauth::loopback::LoopbackServer::bind_port(53682).await?;
                let redirect = listener.ipv4_redirect_uri();
                let verifier = random();
                let state = random();
                let mut url = url::Url::parse("https://www.dropbox.com/oauth2/authorize")
                    .map_err(|_| invalid("Invalid OAuth endpoint"))?;
                url.query_pairs_mut().extend_pairs([
                    ("client_id", key.as_str()),
                    ("response_type", "code"),
                    ("redirect_uri", redirect.as_str()),
                    ("state", state.as_str()),
                    ("code_challenge", pkce(&verifier).as_str()),
                    ("code_challenge_method", "S256"),
                    ("token_access_type", "offline"),
                    ("scope", "files.content.write sharing.write"),
                ]);
                open_url(url.into())
                    .map_err(|_| invalid("Could not open Dropbox authorization"))?;
                let code = listener
                    .wait_for_code(Duration::from_secs(300))
                    .await
                    .map_err(|_| invalid("Dropbox sign-in timed out or was cancelled"))?;
                if code.state.as_deref() != Some(state.as_str()) {
                    return Err(invalid("Dropbox OAuth state mismatch"));
                }
                let value = token(&[
                    ("grant_type", "authorization_code".into()),
                    ("code", code.code),
                    ("redirect_uri", redirect),
                    ("client_id", key.clone()),
                    ("code_verifier", verifier),
                ])
                .await?;
                let refresh = value
                    .get("refresh_token")
                    .and_then(Value::as_str)
                    .ok_or_else(|| invalid("Dropbox did not grant offline access"))?
                    .into();
                let bundle = DropboxBundle {
                    app_key: key,
                    refresh,
                    access: value["access_token"].as_str().unwrap_or_default().into(),
                    expires_at: crate::models::now_ms()
                        + value["expires_in"]
                            .as_i64()
                            .unwrap_or(14400)
                            .clamp(0, 86400)
                            * 1000,
                };
                credentials::store_async(
                    self.credentials.clone(),
                    0,
                    Slot::DropboxOAuth,
                    serde_json::to_string(&bundle)?,
                )
                .await?;
            }
        }
        Ok(())
    }
    pub(super) async fn cloud_access_token(&self, options: &LinkOptions) -> Result<String> {
        match options.provider {
            CloudProvider::GoogleDrive => {
                let id = options.storage_account_id;
                let account = self
                    .db
                    .read(move |conn| crate::db::repo::accounts::get(conn, id))
                    .await?
                    .ok_or_else(|| invalid("Cloud storage account not found"))?;
                if account.provider != Provider::Gmail {
                    return Err(invalid("Google Drive needs a Gmail storage account"));
                }
                self.tokens
                    .access_token(id, Provider::Gmail)
                    .await
                    .map_err(|_| invalid("Reconnect Google Drive in cloud sign-in settings"))
            }
            CloudProvider::OneDrive => {
                let id = options.storage_account_id;
                let account = self
                    .db
                    .read(move |conn| crate::db::repo::accounts::get(conn, id))
                    .await?
                    .ok_or_else(|| invalid("Cloud storage account not found"))?;
                if account.provider != Provider::Microsoft {
                    return Err(invalid("OneDrive needs a Microsoft storage account"));
                }
                self.tokens
                    .access_token_for_scope(id, Provider::Microsoft, ONEDRIVE_SCOPE)
                    .await
                    .map_err(|_| invalid("Reconnect OneDrive in cloud sign-in settings"))
            }
            CloudProvider::Dropbox => {
                let stored =
                    credentials::load_async(self.credentials.clone(), 0, Slot::DropboxOAuth)
                        .await
                        .map_err(|_| invalid("Connect Dropbox in cloud sign-in settings"))?;
                let mut bundle: DropboxBundle =
                    serde_json::from_str(&stored).map_err(|_| invalid("Reconnect Dropbox"))?;
                if bundle.expires_at - crate::models::now_ms() > 60_000 {
                    return Ok(bundle.access);
                }
                let value = token(&[
                    ("grant_type", "refresh_token".into()),
                    ("refresh_token", bundle.refresh.clone()),
                    ("client_id", bundle.app_key.clone()),
                ])
                .await?;
                bundle.access = value["access_token"].as_str().unwrap_or_default().into();
                if let Some(refresh) = value["refresh_token"].as_str() {
                    bundle.refresh = refresh.into();
                }
                bundle.expires_at = crate::models::now_ms()
                    + value["expires_in"]
                        .as_i64()
                        .unwrap_or(14400)
                        .clamp(0, 86400)
                        * 1000;
                credentials::store_async(
                    self.credentials.clone(),
                    0,
                    Slot::DropboxOAuth,
                    serde_json::to_string(&bundle)?,
                )
                .await?;
                Ok(bundle.access)
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pkce_rfc7636() {
        assert_eq!(
            pkce("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }
}
