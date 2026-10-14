use anyhow::{anyhow, Result};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::time::{sleep, Duration};
use tracing::{debug, error, info, warn};

use crate::api::dto::group::KeycloakGroup;
use crate::api::dto::organization::{KeycloakOrganization, OrganizationDomain};
use crate::config::Config;
use crate::models::keycloak::*;
use std::collections::HashMap;

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RawKeycloakGroup {
    pub id: String,
    pub name: String,
    pub path: String,
    pub attributes: Option<HashMap<String, Vec<String>>>,
    pub description: Option<String>,
}

#[derive(Debug, Clone)]
pub struct KeycloakService {
    client: Client,
    config: Config,
}

impl KeycloakService {
    pub fn new(config: Config) -> Self {
        let client = Client::builder()
            .danger_accept_invalid_certs(true)
            .build()
            .expect("Failed to create reqwest client");

        Self { client, config }
    }

    /// Obtain an admin access token using client credentials flow
    pub async fn get_admin_token(&self) -> Result<String> {
        let url = format!(
            "{}/realms/{}/protocol/openid-connect/token",
            self.config.keycloak.url, self.config.keycloak.realm
        );

        let params = [
            ("grant_type", "client_credentials"),
            ("client_id", &self.config.keycloak.client_id),
            ("client_secret", &self.config.keycloak.client_secret),
        ];

        let response = self.client.post(&url).form(&params).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await?;
            error!(
                status = %status,
                "Failed to obtain admin token: {}", error_text
            );
            return Err(anyhow!(
                "Failed to obtain admin token (status: {}): {}",
                status,
                error_text
            ));
        }

        let token_response: TokenResponse = response.json().await?;
        Ok(token_response.access_token)
    }

    /// Create a new organization
    pub async fn create_organization(
        &self,
        access_token: &str,
        name: &str,
        domains: Vec<crate::api::dto::organization::OrganizationDomainRequest>,
        redirect_url: String,
        enabled: String,
        attributes: Option<std::collections::HashMap<String, Vec<String>>>,
    ) -> Result<KeycloakOrganization> {
        let url = format!(
            "{}/admin/realms/{}/organizations",
            self.config.keycloak.url, self.config.keycloak.realm
        );
        info!(url = %url, "Constructed Keycloak URL");

        let mut new_attributes = attributes.clone().unwrap_or_default();
        new_attributes.insert("displayName".to_string(), vec![name.to_string()]);

        let payload = json!({
            "name": name,
            "domains": &domains,
            "redirectUrl": &redirect_url,
            "enabled": enabled == "true",
            "attributes": &new_attributes,
        });
        info!(payload = %payload, "Sending organization create payload to Keycloak");

        let response = self
            .client
            .post(&url)
            .bearer_auth(access_token)
            .json(&payload)
            .send()
            .await?;

        match response.status() {
            StatusCode::CREATED => {
                if let Some(location) = response.headers().get(reqwest::header::LOCATION) {
                    let location_str = location.to_str()?;
                    if let Some(id) = location_str.split('/').next_back() {
                        let created_org = KeycloakOrganization {
                            id: id.to_string(),
                            name: name.to_string(),
                            alias: None,
                            enabled: enabled == "true",
                            description: None,
                            redirect_url: Some(redirect_url),
                            domains: Some(
                                domains
                                    .into_iter()
                                    .map(|d| OrganizationDomain {
                                        name: d.name,
                                        verified: None,
                                    })
                                    .collect(),
                            ),
                            attributes: Some(
                                serde_json::to_value(new_attributes).unwrap_or_default(),
                            ),
                        };
                        info!(org_id = %id, "Successfully created organization in Keycloak");
                        Ok(created_org)
                    } else {
                        Err(anyhow!(
                            "Failed to parse organization ID from Location header"
                        ))
                    }
                } else {
                    Err(anyhow!(
                        "Keycloak returned 201 Created but no Location header was found"
                    ))
                }
            }
            StatusCode::NO_CONTENT => {
                tracing::warn!("Keycloak returned 204 No Content on organization creation, ID is not available.");
                let created_org = KeycloakOrganization {
                    id: String::new(),
                    name: name.to_string(),
                    alias: None,
                    enabled: enabled == "true",
                    description: None,
                    redirect_url: Some(redirect_url),
                    domains: Some(
                        domains
                            .into_iter()
                            .map(|d| OrganizationDomain {
                                name: d.name,
                                verified: None,
                            })
                            .collect(),
                    ),
                    attributes: Some(serde_json::to_value(new_attributes).unwrap_or_default()),
                };
                Ok(created_org)
            }
            _ => {
                let status = response.status();
                let error_text = response.text().await?;
                error!(status = %status, "Failed to create organization: {}", error_text);
                Err(anyhow!(
                    "Failed to create organization (status: {}): {}",
                    status,
                    error_text
                ))
            }
        }
    }

    /// Get all organizations
    pub async fn get_organizations(&self, access_token: &str) -> Result<Vec<KeycloakOrganization>> {
        let url = format!(
            "{}/admin/realms/{}/organizations?briefRepresentation=false",
            self.config.keycloak.url, self.config.keycloak.realm
        );
        info!(url = %url, "Constructed Keycloak URL");

        let response = self
            .client
            .get(&url)
            .bearer_auth(access_token)
            .send()
            .await?
            .error_for_status()?;

        let response_text = response.text().await?;
        tracing::warn!("Raw Keycloak response: {}", response_text);

        let mut orgs: Vec<KeycloakOrganization> = serde_json::from_str(&response_text)?;
        for org in &mut orgs {
            if let Some(attributes) = org.attributes.as_mut() {
                if let Some(serde_json::Value::Array(display_name_vec)) =
                    attributes.get("displayName")
                {
                    if let Some(serde_json::Value::String(display_name)) = display_name_vec.first()
                    {
                        org.name = display_name.clone();
                    }
                }
            }
        }
        tracing::warn!("Deserialized organizations: {:?}", orgs);
        Ok(orgs)
    }

    /// Get a specific organization by ID
    pub async fn get_organization(
        &self,
        access_token: &str,
        org_id: &str,
    ) -> Result<KeycloakOrganization> {
        let url = format!(
            "{}/admin/realms/{}/organizations/{}?briefRepresentation=false",
            self.config.keycloak.url, self.config.keycloak.realm, org_id
        );
        info!(url = %url, "Constructed Keycloak URL");

        let response = self
            .client
            .get(&url)
            .bearer_auth(access_token)
            .send()
            .await?
            .error_for_status()?;

        let response_text = response.text().await?;
        tracing::warn!("Raw single organization response: {}", response_text);

        let mut org: KeycloakOrganization = serde_json::from_str(&response_text)?;
        if let Some(attributes) = org.attributes.as_mut() {
            if let Some(serde_json::Value::Array(display_name_vec)) = attributes.get("displayName")
            {
                if let Some(serde_json::Value::String(display_name)) = display_name_vec.first() {
                    org.name = display_name.clone();
                }
            }
        }
        tracing::warn!("Deserialized single organization: {:?}", org);
        Ok(org)
    }

    /// Update an organization
    pub async fn update_organization(
        &self,
        access_token: &str,
        org_id: &str,
        name: &str,
        domains: Vec<crate::api::dto::organization::OrganizationDomainRequest>,
        attributes: Option<std::collections::HashMap<String, Vec<String>>>,
    ) -> Result<()> {
        let url = format!(
            "{}/admin/realms/{}/organizations/{}",
            self.config.keycloak.url, self.config.keycloak.realm, org_id
        );
        info!(url = %url, "Constructed Keycloak URL");

        let mut new_attributes = attributes.unwrap_or_default();
        // The 'name' field is treated as an immutable alias by Keycloak on update.
        // To change the display name, we set it as a 'displayName' attribute.
        new_attributes.insert("displayName".to_string(), vec![name.to_string()]);

        let payload = json!({
            "domains": domains,
            "attributes": new_attributes,
        });

        let response = self
            .client
            .put(&url)
            .bearer_auth(access_token)
            .json(&payload)
            .send()
            .await?;

        match response.status() {
            StatusCode::NO_CONTENT => Ok(()),
            _ => {
                let status = response.status();
                let error_text = response.text().await?;
                error!(status = %status, "Failed to update organization: {}", error_text);
                Err(anyhow!(
                    "Failed to update organization (status: {}): {}",
                    status,
                    error_text
                ))
            }
        }
    }

    /// Delete an organization
    pub async fn delete_organization(&self, access_token: &str, org_id: &str) -> Result<()> {
        let url = format!(
            "{}/admin/realms/{}/organizations/{}",
            self.config.keycloak.url, self.config.keycloak.realm, org_id
        );
        info!(url = %url, "Constructed Keycloak URL");

        let response = self
            .client
            .delete(&url)
            .bearer_auth(access_token)
            .send()
            .await?;

        match response.status() {
            StatusCode::NO_CONTENT => Ok(()),
            _ => {
                let status = response.status();
                let error_text = response.text().await?;
                error!(status = %status, "Failed to delete organization: {}", error_text);
                Err(anyhow!(
                    "Failed to delete organization (status: {}): {}",
                    status,
                    error_text
                ))
            }
        }
    }

    /// Find a user by username or email
    pub async fn find_user_by_username_or_email(
        &self,
        token: &str,
        query: &str,
    ) -> Result<Option<KeycloakUser>> {
        let url = format!(
            "{}/admin/realms/{}/users?search={}",
            self.config.keycloak.url, self.config.keycloak.realm, query
        );
        let response = self
            .client
            .get(&url)
            .bearer_auth(token)
            .send()
            .await?
            .error_for_status()?;
        let users: Vec<KeycloakUser> = response.json().await?;
        // Try to find an exact match by username or email
        let user = users
            .into_iter()
            .find(|u| u.username == query || u.email == query);
        Ok(user)
    }

    /// Assign a realm role to a user by role name
    pub async fn assign_realm_role_to_user(
        &self,
        token: &str,
        user_id: &str,
        role_name: &str,
    ) -> Result<()> {
        // Get the role object by name
        let url = format!(
            "{}/admin/realms/{}/roles/{}",
            self.config.keycloak.url, self.config.keycloak.realm, role_name
        );
        let role: serde_json::Value = self
            .client
            .get(&url)
            .bearer_auth(token)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        // Assign the role to the user
        let assign_url = format!(
            "{}/admin/realms/{}/users/{}/role-mappings/realm",
            self.config.keycloak.url, self.config.keycloak.realm, user_id
        );
        let roles_payload = serde_json::json!([role]);
        let response = self
            .client
            .post(&assign_url)
            .bearer_auth(token)
            .json(&roles_payload)
            .send()
            .await?;
        match response.status() {
            StatusCode::NO_CONTENT | StatusCode::OK | StatusCode::CREATED => Ok(()),
            _ => {
                let error_text = response.text().await?;
                error!("Failed to assign realm role to user: {}", error_text);
                Err(anyhow!(
                    "Failed to assign realm role to user: {}",
                    error_text
                ))
            }
        }
    }

    /// Assign a client role to a user by role name
    pub async fn assign_client_role_to_user(
        &self,
        token: &str,
        user_id: &str,
        client_id: &str,
        role_name: &str,
    ) -> Result<()> {
        // 1. Get the client's internal ID
        let clients_url = format!(
            "{}/admin/realms/{}/clients?clientId={}",
            self.config.keycloak.url, self.config.keycloak.realm, client_id
        );
        let clients: Vec<serde_json::Value> = self
            .client
            .get(&clients_url)
            .bearer_auth(token)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        let client = clients
            .into_iter()
            .find(|c| c["clientId"] == client_id)
            .ok_or_else(|| anyhow!("Could not find '{}' client", client_id))?;
        let internal_client_id = client["id"]
            .as_str()
            .ok_or_else(|| anyhow!("Client ID is not a string"))?;

        // 2. Get the role object by name from the client
        let role_url = format!(
            "{}/admin/realms/{}/clients/{}/roles/{}",
            self.config.keycloak.url, self.config.keycloak.realm, internal_client_id, role_name
        );
        let role: serde_json::Value = self
            .client
            .get(&role_url)
            .bearer_auth(token)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        // 3. Assign the role to the user
        let assign_url = format!(
            "{}/admin/realms/{}/users/{}/role-mappings/clients/{}",
            self.config.keycloak.url, self.config.keycloak.realm, user_id, internal_client_id
        );
        let roles_payload = serde_json::json!([role]);

        let response = self
            .client
            .post(&assign_url)
            .bearer_auth(token)
            .json(&roles_payload)
            .send()
            .await?;

        match response.status() {
            StatusCode::NO_CONTENT | StatusCode::OK | StatusCode::CREATED => Ok(()),
            _ => {
                let error_text = response.text().await?;
                error!(
                    "Failed to assign client role '{}' to user: {}",
                    role_name, error_text
                );
                Err(anyhow!(
                    "Failed to assign client role '{}' to user: {}",
                    role_name,
                    error_text
                ))
            }
        }
    }


    /// Create an invitation to an organization.
    /// Uses the official Keycloak endpoint:
    /// POST /admin/realms/{realm}/organizations/{org-id}/members/invite-user
    /// This ensures invitations appear in GET /organizations/{org-id}/invitations.
    pub async fn create_invitation(
        &self,
        token: &str,
        org_id: &str,
        email: &str,
        first_name: Option<&str>,
        last_name: Option<&str>,
        _roles: Vec<String>,
        _expiration: Option<String>,
    ) -> Result<KeycloakInvitation> {
        let url = format!(
            "{}/admin/realms/{}/organizations/{}/members/invite-user",
            self.config.keycloak.url, self.config.keycloak.realm, org_id
        );

        let mut form_data: Vec<(&str, String)> = vec![("email", email.to_string())];
        if let Some(first) = first_name {
            if !first.is_empty() {
                form_data.push(("firstName", first.to_string()));
            }
        }
        if let Some(last) = last_name {
            if !last.is_empty() {
                form_data.push(("lastName", last.to_string()));
            }
        }

        info!(url = %url, email = %email, org_id = %org_id, "Creating organization invitation via invite-user endpoint");

        let response = self
            .client
            .post(&url)
            .bearer_auth(token)
            .form(&form_data)
            .send()
            .await?;

        match response.status() {
            s if s.is_success() || s.as_u16() == 204 => {
                let invitation = KeycloakInvitation {
                    id: format!("invitation-{}", chrono::Utc::now().timestamp()),
                    email: email.to_string(),
                    invited_at: chrono::Utc::now().to_rfc3339(),
                    expiration: None,
                    roles: vec![],
                };
                info!(email = %email, "Organization invitation created successfully");
                Ok(invitation)
            }
            _ => {
                let status = response.status();
                let error_text = response.text().await?;
                error!(status = %status, "Failed to create invitation: {}", error_text);
                Err(anyhow!("Failed to create invitation (status: {}): {}", status, error_text))
            }
        }
    }




    /// Get pending invitations for an organization.
    ///
    /// Pending = (users tagged with `org.ro.active = org_id`) MINUS (active org members).
    /// A user is "pending" until they appear in the org's member list. Their status is
    /// derived from Keycloak's own `emailVerified` flag:
    /// - `false` -> awaiting email verification
    /// - `true`  -> email verified but org invitation not yet accepted
    pub async fn get_organization_invitations(
        &self,
        token: &str,
        org_id: &str,
    ) -> Result<Vec<crate::api::dto::invitation::PendingInvitation>> {
        let invited = self.find_invited_users(token, org_id).await?;
        let active = self.get_organization_members(token, org_id).await.unwrap_or_default();

        let active_ids: std::collections::HashSet<String> =
            active.into_iter().map(|m| m.id).collect();

        let pending: Vec<crate::api::dto::invitation::PendingInvitation> = invited
            .into_iter()
            .filter(|u| !active_ids.contains(&u.id))
            .map(|u| crate::api::dto::invitation::PendingInvitation {
                id: u.id,
                email: u.email,
                first_name: u.first_name,
                last_name: u.last_name,
                sent_date: None,
                expires_at: None,
                status: Some(if u.email_verified {
                    "email_verified".to_string()
                } else {
                    "pending".to_string()
                }),
            })
            .collect();

        tracing::info!(org_id = %org_id, count = pending.len(), "Computed pending invitations");
        Ok(pending)
    }

    /// Find all users tagged with `org.ro.active = org_id` via Keycloak's `q` attribute search.
    async fn find_invited_users(&self, token: &str, org_id: &str) -> Result<Vec<KeycloakUser>> {
        let url = format!(
            "{}/admin/realms/{}/users?q={}:{}&briefRepresentation=false&max=1000",
            self.config.keycloak.url, self.config.keycloak.realm, ORG_ACTIVE_ATTR, org_id
        );

        tracing::info!(url = %url, org_id = %org_id, "Searching invited users by attribute");

        let resp = self.client.get(&url).bearer_auth(token).send().await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            tracing::error!(status = %status, body = %body, "Attribute search for invited users failed");
            return Ok(vec![]);
        }

        let users: Vec<KeycloakUser> = resp.json().await?;
        tracing::info!(count = users.len(), "Invited users found by attribute search");
        Ok(users)
    }


    /// Delete a pending invitation by ID
    /// DELETE /admin/realms/{realm}/organizations/{org-id}/invitations/{id}
    pub async fn delete_organization_invitation(
        &self,
        token: &str,
        org_id: &str,
        invitation_id: &str,
    ) -> Result<()> {
        let url = format!(
            "{}/admin/realms/{}/organizations/{}/invitations/{}",
            self.config.keycloak.url, self.config.keycloak.realm, org_id, invitation_id
        );

        let resp = self.client.delete(&url).bearer_auth(token).send().await?;

        match resp.status() {
            s if s.is_success() || s.as_u16() == 204 => Ok(()),
            s if s.as_u16() == 404 => {
                tracing::warn!(invitation_id = %invitation_id, "Invitation not found, may already be deleted");
                Ok(())
            }
            _ => {
                let err = resp.text().await.unwrap_or_default();
                Err(anyhow::anyhow!("Failed to delete invitation: {}", err))
            }
        }
    }

    /// Resend a pending invitation by ID
    /// POST /admin/realms/{realm}/organizations/{org-id}/invitations/{id}/resend
    pub async fn resend_organization_invitation(
        &self,
        token: &str,
        org_id: &str,
        invitation_id: &str,
    ) -> Result<()> {
        let url = format!(
            "{}/admin/realms/{}/organizations/{}/invitations/{}/resend",
            self.config.keycloak.url, self.config.keycloak.realm, org_id, invitation_id
        );

        let resp = self.client.post(&url).bearer_auth(token).send().await?;

        match resp.status() {
            s if s.is_success() || s.as_u16() == 204 => Ok(()),
            s if s.as_u16() == 404 => {
                Err(anyhow::anyhow!("Invitation not found"))
            }
            _ => {
                let err = resp.text().await.unwrap_or_default();
                Err(anyhow::anyhow!("Failed to resend invitation: {}", err))
            }
        }
    }

    /// Create a new user with email verification required
    pub async fn create_user_with_email_verification(
        &self,
        token: &str,
        request: &CreateUserRequest,
    ) -> Result<KeycloakUser> {
        let url = format!(
            "{}/admin/realms/{}/users",
            self.config.keycloak.url, self.config.keycloak.realm
        );

        // Generate a temporary password
        let temp_password = self.generate_temporary_password();

        let mut payload = json!({
            "username": request.username,
            "email": request.email,
            "enabled": true,
            "emailVerified": false,
            "requiredActions": ["VERIFY_EMAIL"],
            "credentials": [{
                "type": "password",
                "value": temp_password,
                "temporary": true
            }]
        });

        // Add optional fields if provided
        if let Some(first_name) = &request.first_name {
            payload["firstName"] = json!(first_name);
        }
        if let Some(last_name) = &request.last_name {
            payload["lastName"] = json!(last_name);
        }
        if let Some(attributes) = &request.attributes {
            payload["attributes"] = json!(attributes);
        }

        info!(url = %url, email = %request.email, "Creating user with email verification");

        let response = self
            .client
            .post(&url)
            .bearer_auth(token)
            .json(&payload)
            .send()
            .await?;

        match response.status() {
            StatusCode::CREATED => {
                // Get the created user to return the full user object
                let location = response
                    .headers()
                    .get("location")
                    .and_then(|h| h.to_str().ok())
                    .ok_or_else(|| anyhow!("No location header in response"))?;

                let user_id = location
                    .split('/')
                    .next_back()
                    .ok_or_else(|| anyhow!("Invalid location header"))?;

                // Fetch the created user
                let user = self.get_user_by_id(token, user_id).await?;

                // Try to trigger email verification email, but don't fail if it doesn't work
                match self.trigger_email_verification(token, user_id, None).await {
                    Ok(_) => {
                        info!(user_id = %user_id, "Email verification email sent successfully");
                    }
                    Err(e) => {
                        warn!(user_id = %user_id, error = %e, "Failed to send email verification email, but user was created successfully");
                        // Don't fail the entire operation, just log the warning
                    }
                }

                info!(user_id = %user_id, email = %request.email, "User created successfully with email verification required");
                Ok(user)
            }
            _ => {
                let error_text = response.text().await?;
                error!("Failed to create user: {}", error_text);
                Err(anyhow!("Failed to create user: {}", error_text))
            }
        }
    }

    /// Update user attributes (e.g., to store dimension permissions)
    pub async fn update_user_attributes(
        &self,
        token: &str,
        user_id: &str,
        attributes: serde_json::Value,
        email: Option<&str>,
    ) -> Result<()> {
        let url = format!(
            "{}/admin/realms/{}/users/{}",
            self.config.keycloak.url, self.config.keycloak.realm, user_id
        );

        // Fetch existing user to merge attributes (Keycloak PUT replaces all attributes)
        let existing_user: serde_json::Value = self
            .client
            .get(&url)
            .bearer_auth(token)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        // Merge new attributes into existing ones
        let mut merged_attributes = existing_user
            .get("attributes")
            .and_then(|a| a.as_object())
            .cloned()
            .unwrap_or_default();

        if let Some(new_attrs) = attributes.as_object() {
            for (k, v) in new_attrs {
                merged_attributes.insert(k.clone(), v.clone());
            }
        }

        let mut payload = existing_user.clone();
        payload["attributes"] = serde_json::Value::Object(merged_attributes);

        if let Some(email) = email {
            payload["email"] = json!(email);
        }

        info!(url = %url, user_id = %user_id, "Updating user attributes");

        let response = self
            .client
            .put(&url)
            .bearer_auth(token)
            .json(&payload)
            .send()
            .await?;

        match response.status() {
            StatusCode::NO_CONTENT => Ok(()),
            StatusCode::OK => Ok(()),
            _ => {
                let status = response.status();
                let error_text = response.text().await?;
                error!(status = %status, "Failed to update user attributes: {}", error_text);
                Err(anyhow!(
                    "Failed to update user attributes (status: {}): {}",
                    status,
                    error_text
                ))
            }
        }
    }

    /// One-time migration: copy legacy `invited_organization` attribute values onto
    /// `org.ro.active` so previously-invited users are picked up by the new pending
    /// invitation search. Returns the number of users migrated.
    pub async fn migrate_legacy_invitation_attribute(&self, token: &str) -> Result<usize> {
        let url = format!(
            "{}/admin/realms/{}/users?max=1000&briefRepresentation=false",
            self.config.keycloak.url, self.config.keycloak.realm
        );

        let resp = self.client.get(&url).bearer_auth(token).send().await?;
        if !resp.status().is_success() {
            return Err(anyhow!("Failed to list users: {}", resp.status()));
        }

        let users: Vec<KeycloakUser> = resp.json().await?;
        let mut migrated = 0usize;

        for mut user in users {
            if user.attributes.is_none() {
                if let Ok(full) = self.get_user_by_id(token, &user.id).await {
                    user.attributes = full.attributes;
                }
            }

            let Some(attrs) = &user.attributes else { continue };
            let Some(legacy) = attrs.get("invited_organization") else { continue };
            let has_value = legacy
                .as_array()
                .map(|a| !a.is_empty())
                .unwrap_or_else(|| legacy.is_string());

            if !has_value {
                continue;
            }

            let mut new_attrs = json!({ ORG_ACTIVE_ATTR: legacy.clone() });
            new_attrs["invited_organization"] = json!([]);

            self.update_user_attributes(token, &user.id, new_attrs, Some(&user.email))
                .await?;
            migrated += 1;
            info!(user_id = %user.id, "Migrated legacy invitation attribute to {}", ORG_ACTIVE_ATTR);
        }

        Ok(migrated)
    }

    /// Get user by ID
    pub async fn get_user_by_id(&self, token: &str, user_id: &str) -> Result<KeycloakUser> {
        let url = format!(
            "{}/admin/realms/{}/users/{}",
            self.config.keycloak.url, self.config.keycloak.realm, user_id
        );

        let response = self.client.get(&url).bearer_auth(token).send().await?;

        match response.status() {
            StatusCode::OK => {
                let user: KeycloakUser = response.json().await?;
                Ok(user)
            }
            _ => {
                let error_text = response.text().await?;
                error!("Failed to get user by ID: {}", error_text);
                Err(anyhow!("Failed to get user by ID: {}", error_text))
            }
        }
    }

    /// Get user groups by user ID
    pub async fn get_user_groups(&self, token: &str, user_id: &str) -> Result<Vec<KeycloakGroup>> {
        let url = format!(
            "{}/admin/realms/{}/users/{}/groups",
            self.config.keycloak.url, self.config.keycloak.realm, user_id
        );

        let response = self.client.get(&url).bearer_auth(token).send().await?.error_for_status()?;
        let raw_groups: Vec<RawKeycloakGroup> = response.json().await?;
        let groups = raw_groups
            .into_iter()
            .map(|g| KeycloakGroup {
                id: g.id.clone(),
                name: g.name.clone(),
                path: g.path.clone(),
                description: g.description.clone(),
            })
            .collect();
        Ok(groups)
    }

    /// Get user organizations by user ID
    pub async fn get_user_organizations(&self, token: &str, user_id: &str) -> Result<Vec<KeycloakOrganization>> {
        let url = format!(
            "{}/admin/realms/{}/organizations/members/{}/organizations",
            self.config.keycloak.url, self.config.keycloak.realm, user_id
        );

        let response = self.client.get(&url).bearer_auth(token).send().await?.error_for_status()?;
        let orgs: Vec<KeycloakOrganization> = response.json().await?;
        Ok(orgs)
    }

    /// Generate a temporary password for new users
    fn generate_temporary_password(&self) -> String {
        use rand::distributions::Alphanumeric;
        use rand::{thread_rng, Rng};

        let mut rng = thread_rng();
        let password: String = (0..12).map(|_| rng.sample(Alphanumeric) as char).collect();

        format!("Temp{}!", password)
    }

    /// Trigger email verification email for a user
    pub async fn trigger_email_verification(
        &self,
        token: &str,
        user_id: &str,
        redirect_uri: Option<&str>,
    ) -> Result<()> {
        // Method 1: Try the send-verify-email endpoint
        let url = if let Some(uri) = redirect_uri {
            format!(
                "{}/admin/realms/{}/users/{}/send-verify-email?redirect_uri={}&client_id={}",
                self.config.keycloak.url,
                self.config.keycloak.realm,
                user_id,
                uri,
                self.config.keycloak.client_id
            )
        } else {
            format!(
                "{}/admin/realms/{}/users/{}/send-verify-email",
                self.config.keycloak.url, self.config.keycloak.realm, user_id
            )
        };

        let response = self.client.post(&url).bearer_auth(token).send().await;

        if let Ok(response) = response {
            if response.status() == StatusCode::NO_CONTENT || response.status() == StatusCode::OK {
                info!(user_id = %user_id, "Email verification email triggered successfully via send-verify-email");
                return Ok(());
            }
            if let Ok(error_text) = response.text().await {
                debug!("send-verify-email failed: {}", error_text);
            }
        }

        // Method 2: Try executing the VERIFY_EMAIL action
        let url = if let Some(uri) = redirect_uri {
            format!(
                "{}/admin/realms/{}/users/{}/execute-actions-email?redirect_uri={}&client_id={}",
                self.config.keycloak.url,
                self.config.keycloak.realm,
                user_id,
                uri,
                self.config.keycloak.client_id
            )
        } else {
            format!(
                "{}/admin/realms/{}/users/{}/execute-actions-email",
                self.config.keycloak.url, self.config.keycloak.realm, user_id
            )
        };

        let payload = json!(["VERIFY_EMAIL"]);

        let response = self
            .client
            .put(&url)
            .bearer_auth(token)
            .json(&payload)
            .send()
            .await?;

        match response.status() {
            StatusCode::NO_CONTENT | StatusCode::OK => {
                info!(user_id = %user_id, "Email verification email triggered successfully via execute-actions-email");
                Ok(())
            }
            _ => {
                let error_text = response.text().await?;
                error!(
                    "Failed to trigger email verification via execute-actions-email: {}",
                    error_text
                );
                Err(anyhow!(
                    "Failed to trigger email verification: {}",
                    error_text
                ))
            }
        }
    }

    /// Get all members of an organization
    pub async fn get_organization_members(
        &self,
        token: &str,
        org_id: &str,
    ) -> Result<Vec<KeycloakUser>> {
        let url = format!(
            "{}/admin/realms/{}/organizations/{}/members",
            self.config.keycloak.url, self.config.keycloak.realm, org_id
        );

        info!(url = %url, org_id = %org_id, "Getting organization members");

        let response = self.client.get(&url).bearer_auth(token).send().await?;

        match response.status() {
            StatusCode::OK => {
                let users: Vec<KeycloakUser> = response.json().await?;
                info!(org_id = %org_id, count = users.len(), "Successfully retrieved organization members");
                Ok(users)
            }
            _ => {
                let error_text = response.text().await?;
                error!("Failed to get organization members: {}", error_text);
                Err(anyhow!(
                    "Failed to get organization members: {}",
                    error_text
                ))
            }
        }
    }

    /// Waits for a user to be available via the Keycloak Admin API.
    async fn wait_for_user_to_be_available(&self, token: &str, user_id: &str) -> Result<()> {
        let max_attempts = 10;
        let mut delay = Duration::from_secs(1);
        for attempt in 1..=max_attempts {
            info!(user_id = %user_id, attempt = attempt, "Checking if user is available in Keycloak");
            match self.get_user_by_id(token, user_id).await {
                Ok(_) => {
                    info!(user_id = %user_id, "User is available in Keycloak.");
                    return Ok(());
                }
                Err(e) => {
                    warn!(user_id = %user_id, error = %e, "User not yet available. Retrying in {:?}...", delay);
                    sleep(delay).await;
                    delay *= 2; // Exponential backoff
                }
            }
        }
        Err(anyhow!(
            "User {} was not available after {} attempts",
            user_id,
            max_attempts
        ))
    }

    /// Add a user to an organization
    pub async fn add_user_to_organization(
        &self,
        token: &str,
        org_id: &str,
        user_id: &str,
    ) -> Result<()> {
        // First, wait until the user is actually available via the API
        self.wait_for_user_to_be_available(token, user_id).await?;

        let url = format!(
            "{}/admin/realms/{}/organizations/{}/members",
            self.config.keycloak.url, self.config.keycloak.realm, org_id
        );

        info!(url = %url, user_id = %user_id, org_id = %org_id, "Attempting to add user to organization with raw user ID in body");

        let response = self
            .client
            .post(&url)
            .bearer_auth(token)
            .header("Content-Type", "application/json")
            .body(format!("\"{}\"", user_id)) // Send as a JSON string literal
            .send()
            .await?;

        match response.status() {
            StatusCode::NO_CONTENT | StatusCode::CREATED => {
                info!(user_id = %user_id, org_id = %org_id, "Successfully added user to organization");
                Ok(())
            }
            _ => {
                let status = response.status();
                let error_text = response.text().await?;
                error!(status = %status, "Failed to add user to organization: {}", error_text);
                Err(anyhow!(
                    "Failed to add user to organization (status: {}): {}",
                    status,
                    error_text
                ))
            }
        }
    }
    /// Delete a user by ID
    pub async fn delete_user(&self, token: &str, user_id: &str) -> Result<()> {
        let url = format!(
            "{}/admin/realms/{}/users/{}",
            self.config.keycloak.url, self.config.keycloak.realm, user_id
        );

        info!(url = %url, user_id = %user_id, "Deleting user");

        let response = self.client.delete(&url).bearer_auth(token).send().await?;

        match response.status() {
            StatusCode::NO_CONTENT => {
                info!(user_id = %user_id, "Successfully deleted user");
                Ok(())
            }
            _ => {
                let error_text = response.text().await?;
                error!("Failed to delete user: {}", error_text);
                Err(anyhow!("Failed to delete user: {}", error_text))
            }
        }
    }
    /// Create a new group
    pub async fn create_group(
        &self,
        access_token: &str,
        name: &str,
        description: Option<String>,
    ) -> Result<KeycloakGroup> {
        let url = format!(
            "{}/admin/realms/{}/groups",
            self.config.keycloak.url, self.config.keycloak.realm
        );
        info!(url = %url, "Constructed Keycloak URL for creating group");

        let payload = json!({
            "name": name,
            "description": description,
        });
        info!(payload = %payload, "Sending group create payload to Keycloak");

        let response = self
            .client
            .post(&url)
            .bearer_auth(access_token)
            .json(&payload)
            .send()
            .await?;

        match response.status() {
            StatusCode::CREATED => {
                if let Some(location) = response.headers().get(reqwest::header::LOCATION) {
                    let location_str = location.to_str()?;
                    if let Some(id) = location_str.split('/').next_back() {
                        let created_group = self.get_group_by_id(access_token, id).await?;
                        info!(group_id = %id, "Successfully created group in Keycloak");
                        Ok(created_group)
                    } else {
                        Err(anyhow!("Failed to parse group ID from Location header"))
                    }
                } else {
                    Err(anyhow!(
                        "Keycloak returned 201 Created but no Location header was found"
                    ))
                }
            }
            _ => {
                let status = response.status();
                let error_text = response.text().await?;
                error!(status = %status, "Failed to create group: {}", error_text);
                Err(anyhow!(
                    "Failed to create group (status: {}): {}",
                    status,
                    error_text
                ))
            }
        }
    }

    /// Get groups, optionally filtering by a search term
    pub async fn get_groups(
        &self,
        access_token: &str,
        search: Option<&str>,
    ) -> Result<Vec<KeycloakGroup>> {
        let mut url = format!(
            "{}/admin/realms/{}/groups",
            self.config.keycloak.url, self.config.keycloak.realm
        );
        if let Some(search_term) = search {
            url.push_str(&format!("?search={}", search_term));
        }
        info!(url = %url, "Constructed Keycloak URL for getting groups");

        let response = self
            .client
            .get(&url)
            .bearer_auth(access_token)
            .send()
            .await?
            .error_for_status()?;

        let raw_groups: Vec<RawKeycloakGroup> = response.json().await?;
        let groups = raw_groups
            .into_iter()
            .map(|g| {
                let mut clean_name = g.name.clone();
                let parts: Vec<&str> = g.name.splitn(6, '-').collect();
                if parts.len() == 6 {
                    // Basic check for UUID structure
                    if parts[0].len() == 8
                        && parts[1].len() == 4
                        && parts[2].len() == 4
                        && parts[3].len() == 4
                        && parts[4].len() == 12
                    {
                        clean_name = parts[5].to_string();
                    }
                }
                KeycloakGroup {
                    id: g.id,
                    name: clean_name,
                    path: g.path,
                    description: g.description,
                }
            })
            .collect();
        Ok(groups)
    }

    /// Get a specific group by ID
    pub async fn get_group_by_id(
        &self,
        access_token: &str,
        group_id: &str,
    ) -> Result<KeycloakGroup> {
        let url = format!(
            "{}/admin/realms/{}/groups/{}",
            self.config.keycloak.url, self.config.keycloak.realm, group_id
        );
        info!(url = %url, "Constructed Keycloak URL for getting group by ID");

        let response = self
            .client
            .get(&url)
            .bearer_auth(access_token)
            .send()
            .await?
            .error_for_status()?;

        let raw_group: RawKeycloakGroup = response.json().await?;
        let mut group = KeycloakGroup {
            id: raw_group.id,
            name: raw_group.name.clone(),
            path: raw_group.path,
            description: raw_group.description,
        };

        let parts: Vec<&str> = raw_group.name.splitn(6, '-').collect();
        if parts.len() == 6
            && parts[0].len() == 8
            && parts[1].len() == 4
            && parts[2].len() == 4
            && parts[3].len() == 4
            && parts[4].len() == 12
        {
            group.name = parts[5].to_string();
        }

        Ok(group)
    }

    /// Get a specific group by path
    pub async fn get_group_by_path(
        &self,
        access_token: &str,
        group_path: &str,
    ) -> Result<Option<KeycloakGroup>> {
        let url = format!(
            "{}/admin/realms/{}/groups",
            self.config.keycloak.url, self.config.keycloak.realm
        );
        info!(url = %url, "Constructed Keycloak URL for getting groups");

        let response = self
            .client
            .get(&url)
            .bearer_auth(access_token)
            .send()
            .await?
            .error_for_status()?;

        let raw_groups: Vec<RawKeycloakGroup> = response.json().await?;
        let group = raw_groups
            .into_iter()
            .find(|g| g.path == group_path)
            .map(|g| {
                let mut clean_name = g.name.clone();
                let parts: Vec<&str> = g.name.splitn(6, '-').collect();
                if parts.len() == 6 {
                    // Basic check for UUID structure
                    if parts[0].len() == 8
                        && parts[1].len() == 4
                        && parts[2].len() == 4
                        && parts[3].len() == 4
                        && parts[4].len() == 12
                    {
                        clean_name = parts[5].to_string();
                    }
                }
                KeycloakGroup {
                    id: g.id,
                    name: clean_name,
                    path: g.path,
                    description: g.description,
                }
            });
        Ok(group)
    }

    /// Update a group
    pub async fn update_group(
        &self,
        access_token: &str,
        group_id: &str,
        name: &str,
        description: Option<String>,
    ) -> Result<()> {
        let url = format!(
            "{}/admin/realms/{}/groups/{}",
            self.config.keycloak.url, self.config.keycloak.realm, group_id
        );
        info!(url = %url, "Constructed Keycloak URL for updating group");

        let get_url = format!(
            "{}/admin/realms/{}/groups/{}",
            self.config.keycloak.url, self.config.keycloak.realm, group_id
        );
        let response = self
            .client
            .get(&get_url)
            .bearer_auth(access_token)
            .send()
            .await?
            .error_for_status()?;
        let raw_group: RawKeycloakGroup = response.json().await?;
        let current_name = raw_group.name;

        let parts: Vec<&str> = current_name.splitn(6, '-').collect();
        let new_name = if parts.len() == 6
            && parts[0].len() == 8
            && parts[1].len() == 4
            && parts[2].len() == 4
            && parts[3].len() == 4
            && parts[4].len() == 12
        {
            let uuid_parts = [parts[0], parts[1], parts[2], parts[3], parts[4]];
            let org_id = uuid_parts.join("-");
            format!("{}-{}", org_id, name)
        } else {
            name.to_string()
        };

        let payload = json!({
            "name": new_name,
            "description": description,
        });

        let response = self
            .client
            .put(&url)
            .bearer_auth(access_token)
            .json(&payload)
            .send()
            .await?;

        match response.status() {
            StatusCode::NO_CONTENT => Ok(()),
            _ => {
                let status = response.status();
                let error_text = response.text().await?;
                error!(status = %status, "Failed to update group: {}", error_text);
                Err(anyhow!(
                    "Failed to update group (status: {}): {}",
                    status,
                    error_text
                ))
            }
        }
    }

    /// Delete a group
    pub async fn delete_group(&self, access_token: &str, group_id: &str) -> Result<()> {
        let url = format!(
            "{}/admin/realms/{}/groups/{}",
            self.config.keycloak.url, self.config.keycloak.realm, group_id
        );
        info!(url = %url, "Constructed Keycloak URL for deleting group");

        let response = self
            .client
            .delete(&url)
            .bearer_auth(access_token)
            .send()
            .await?;

        match response.status() {
            StatusCode::NO_CONTENT => Ok(()),
            _ => {
                let status = response.status();
                let error_text = response.text().await?;
                error!(status = %status, "Failed to delete group: {}", error_text);
                Err(anyhow!(
                    "Failed to delete group (status: {}): {}",
                    status,
                    error_text
                ))
            }
        }
    }

    /// Create a new user
    pub async fn create_user(&self, token: &str, request: &CreateUserRequest) -> Result<String> {
        let url = format!(
            "{}/admin/realms/{}/users",
            self.config.keycloak.url, self.config.keycloak.realm
        );

        let payload = json!({
            "username": request.email, // Using email as username as per example
            "email": request.email,
            "enabled": true,
            "emailVerified": false,
            "firstName": request.first_name,
            "lastName": request.last_name,
        });

        info!(url = %url, email = %request.email, "Creating user");

        let response = self
            .client
            .post(&url)
            .bearer_auth(token)
            .json(&payload)
            .send()
            .await?;

        match response.status() {
            StatusCode::CREATED => {
                let location = response
                    .headers()
                    .get("location")
                    .and_then(|h| h.to_str().ok())
                    .ok_or_else(|| anyhow!("No location header in response"))?;

                let user_id = location
                    .split('/')
                    .next_back()
                    .ok_or_else(|| anyhow!("Invalid location header"))?
                    .to_string();

                info!(user_id = %user_id, email = %request.email, "User created successfully");
                Ok(user_id)
            }
            _ => {
                let error_text = response.text().await?;
                error!("Failed to create user: {}", error_text);
                Err(anyhow!("Failed to create user: {}", error_text))
            }
        }
    }

    /// Add a user to a group
    pub async fn add_user_to_group(
        &self,
        token: &str,
        user_id: &str,
        group_id: &str,
    ) -> Result<()> {
        let url = format!(
            "{}/admin/realms/{}/users/{}/groups/{}",
            self.config.keycloak.url, self.config.keycloak.realm, user_id, group_id
        );

        info!(url = %url, user_id = %user_id, group_id = %group_id, "Adding user to group");

        let response = self.client.put(&url).bearer_auth(token).send().await?;

        match response.status() {
            StatusCode::NO_CONTENT => {
                info!(user_id = %user_id, group_id = %group_id, "Successfully added user to group");
                Ok(())
            }
            _ => {
                let error_text = response.text().await?;
                error!("Failed to add user to group: {}", error_text);
                Err(anyhow!("Failed to add user to group: {}", error_text))
            }
        }
    }

    /// Get all members of a group
    pub async fn get_group_members(
        &self,
        token: &str,
        group_id: &str,
    ) -> Result<Vec<KeycloakUser>> {
        let url = format!(
            "{}/admin/realms/{}/groups/{}/members",
            self.config.keycloak.url, self.config.keycloak.realm, group_id
        );

        info!(url = %url, group_id = %group_id, "Getting group members");

        let response = self.client.get(&url).bearer_auth(token).send().await?;

        match response.status() {
            StatusCode::OK => {
                let mut users: Vec<KeycloakUser> = response.json().await?;
                for user in &mut users {
                    // Fetch roles
                    let roles_url = format!(
                        "{}/admin/realms/{}/users/{}/role-mappings",
                        self.config.keycloak.url, self.config.keycloak.realm, user.id
                    );
                    let roles_response = self
                        .client
                        .get(&roles_url)
                        .bearer_auth(token)
                        .send()
                        .await?;
                    if roles_response.status() == StatusCode::OK {
                        let roles: serde_json::Value = roles_response.json().await?;
                        if let Some(real_mappings) = roles.get("realmMappings") {
                            user.roles = Some(real_mappings.clone());
                        }
                    }

                    // Fetch full user details to get attributes (group members endpoint omits them)
                    if let Ok(full_user) = self.get_user_by_id(token, &user.id).await {
                        user.attributes = full_user.attributes;
                    }
                }
                info!(group_id = %group_id, count = users.len(), "Successfully retrieved group members");
                Ok(users)
            }
            _ => {
                let error_text = response.text().await?;
                error!("Failed to get group members: {}", error_text);
                Err(anyhow!("Failed to get group members: {}", error_text))
            }
        }
    }

    /// Update basic user profile information (firstName, lastName)
    pub async fn update_user_profile(
        &self,
        token: &str,
        user_id: &str,
        first_name: Option<String>,
        last_name: Option<String>,
        email: Option<String>,
    ) -> Result<()> {
        let url = format!(
            "{}/admin/realms/{}/users/{}",
            self.config.keycloak.url, self.config.keycloak.realm, user_id
        );

        let mut payload = serde_json::Map::new();
        if let Some(first) = first_name {
            payload.insert("firstName".to_string(), json!(first));
        }
        if let Some(last) = last_name {
            payload.insert("lastName".to_string(), json!(last));
        }
        if let Some(email) = email {
            payload.insert("email".to_string(), json!(email));
        }

        info!(url = %url, user_id = %user_id, "Updating user profile");

        let response = self
            .client
            .put(&url)
            .bearer_auth(token)
            .json(&payload)
            .send()
            .await?;

        match response.status() {
            StatusCode::NO_CONTENT | StatusCode::OK => Ok(()),
            _ => {
                let status = response.status();
                let error_text = response.text().await?;
                error!(status = %status, "Failed to update user profile: {}", error_text);
                Err(anyhow!(
                    "Failed to update user profile (status: {}): {}",
                    status,
                    error_text
                ))
            }
        }
    }

    /// Reset user password (admin action)
    pub async fn reset_password(
        &self,
        token: &str,
        user_id: &str,
        new_password: &str,
        temporary: bool,
    ) -> Result<()> {
        let url = format!(
            "{}/admin/realms/{}/users/{}/reset-password",
            self.config.keycloak.url, self.config.keycloak.realm, user_id
        );

        let payload = json!({
            "type": "password",
            "value": new_password,
            "temporary": temporary,
        });

        info!(url = %url, user_id = %user_id, "Resetting user password");

        let response = self
            .client
            .put(&url)
            .bearer_auth(token)
            .json(&payload)
            .send()
            .await?;

        match response.status() {
            StatusCode::NO_CONTENT | StatusCode::OK => Ok(()),
            _ => {
                let status = response.status();
                let error_text = response.text().await?;
                error!(status = %status, "Failed to reset password: {}", error_text);
                Err(anyhow!(
                    "Failed to reset password (status: {}): {}",
                    status,
                    error_text
                ))
            }
        }
    }

    /// Verify user password by attempting to get a token via direct grant
    pub async fn verify_user_password(
        &self,
        username: &str,
        password: &str,
    ) -> Result<bool> {
        let url = format!(
            "{}/realms/{}/protocol/openid-connect/token",
            self.config.keycloak.url, self.config.keycloak.realm
        );

        let params = [
            ("grant_type", "password"),
            ("client_id", &self.config.keycloak.client_id),
            ("client_secret", &self.config.keycloak.client_secret),
            ("username", username),
            ("password", password),
            ("scope", "openid"),
        ];

        let response = self.client.post(&url).form(&params).send().await?;

        Ok(response.status().is_success())
    }
}
