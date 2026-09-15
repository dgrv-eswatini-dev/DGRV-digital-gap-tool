use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Keycloak user attribute used to track which organisation a user was invited to.
/// Registered in the realm's User Profile schema so it is searchable via `q=`.
pub const ORG_ACTIVE_ATTR: &str = "org.ro.active";

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct KeycloakUser {
    pub id: String,
    pub username: String,
    #[serde(rename = "firstName", default)]
    pub first_name: Option<String>,
    #[serde(rename = "lastName", default)]
    pub last_name: Option<String>,
    pub email: String,
    #[serde(rename = "emailVerified", default)]
    pub email_verified: bool,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub attributes: Option<serde_json::Value>,
    #[serde(default)]
    pub roles: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateUserRequest {
    pub username: String,
    pub email: String,
    #[serde(rename = "firstName")]
    pub first_name: Option<String>,
    #[serde(rename = "lastName")]
    pub last_name: Option<String>,
    #[serde(rename = "emailVerified")]
    pub email_verified: Option<bool>,
    pub enabled: Option<bool>,
    pub attributes: Option<serde_json::Value>,
    pub credentials: Option<Vec<KeycloakCredential>>,
    #[serde(rename = "requiredActions")]
    pub required_actions: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeycloakCredential {
    #[serde(rename = "type")]
    pub credential_type: String,
    pub value: String,
    pub temporary: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeycloakInvitation {
    pub id: String,
    pub email: String,
    pub invited_at: String,
    pub expiration: Option<String>,
    pub roles: Vec<String>,
}
