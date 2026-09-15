use crate::{
    api::dto::member::{AddMemberRequest, AddMemberResponse}, error::AppResult,
    models::keycloak::{CreateUserRequest, ORG_ACTIVE_ATTR},
    AppState,
};
use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};

/// Add a member to a group (cooperation)
#[utoipa::path(
    post,
    path = "/admin/groups/{group_id}/members",
    tag = "User",
    params(
        ("group_id" = String, Path, description = "Cooperation Group ID")
    ),
    request_body = AddMemberRequest,
    responses((status = 201, description = "Created", body = AddMemberResponse))
)]
pub async fn add_member(
    State(state): State<AppState>,
    Extension(_token): Extension<String>,
    Path(group_id): Path<String>,
    Json(payload): Json<AddMemberRequest>,
) -> AppResult<impl IntoResponse> {
    // Find user by email
    tracing::info!(
        "add_member: incoming roles={:?}, dimension_ids={:?}",
        payload.roles,
        payload.dimension_ids
    );

    let admin_token = state.keycloak_service.get_admin_token().await?;

    let existing_user = state
        .keycloak_service
        .find_user_by_username_or_email(&admin_token, &payload.email)
        .await?;

    // Build attributes payload if any dimensions were provided
    let dimension_attrs = payload
        .dimension_ids
        .as_ref()
        .map(|dimension_ids| serde_json::json!({ "assigned_dimensions": dimension_ids }));

    let mut email_sent = true;

    let user_id = if let Some(user) = existing_user {
        // Exclusivity checks
        let groups = state.keycloak_service.get_user_groups(&admin_token, &user.id).await?;
        if groups.iter().any(|g| g.id != group_id) {
            return Err(crate::error::AppError::BadRequest("User is already a member of a cooperative".to_string()));
        }

        let orgs = state.keycloak_service.get_user_organizations(&admin_token, &user.id).await?;
        if !orgs.is_empty() {
            return Err(crate::error::AppError::BadRequest("User is already a member of an organization".to_string()));
        }

        if let Some(attrs) = &user.attributes {
            if let Some(invited) = attrs.get(ORG_ACTIVE_ATTR) {
                if let Some(arr) = invited.as_array() {
                    if !arr.is_empty() {
                        return Err(crate::error::AppError::BadRequest("User is already a member of an organization".to_string()));
                    }
                }
            }
        }

        // If user exists, update attributes when provided
        if let Some(attrs) = &dimension_attrs {
            tracing::info!(
                "add_member: updating existing user {} with attrs={}",
                user.id,
                attrs
            );
            state
                .keycloak_service
                .update_user_attributes(&admin_token, &user.id, attrs.clone(), Some(&payload.email))
                .await?;
        }
        user.id
    } else {
        // Create user if not exists
        let user_request = CreateUserRequest {
            username: payload.email.clone(),
            email: payload.email.clone(),
            first_name: payload.first_name,
            last_name: payload.last_name,
            email_verified: Some(false),
            enabled: Some(true),
            attributes: dimension_attrs.clone(),
            credentials: None,
            required_actions: Some(vec!["VERIFY_EMAIL".to_string()]),
        };

        let new_user = state
            .keycloak_service
            .create_user_with_email_verification(&admin_token, &user_request)
            .await?;

        // Ensure attributes are persisted by explicitly updating after creation
        if let Some(attrs) = &dimension_attrs {
            tracing::info!(
                "add_member: updating newly created user {} with attrs={}",
                new_user.id,
                attrs
            );
            state
                .keycloak_service
                .update_user_attributes(&admin_token, &new_user.id, attrs.clone(), Some(&payload.email))
                .await?;
        }

        // Try to send verification email, track whether it succeeded
        match state
            .keycloak_service
            .trigger_email_verification(&admin_token, &new_user.id, None)
            .await
        {
            Ok(_) => {
                tracing::info!(user_id = %new_user.id, "Verification email sent successfully");
            }
            Err(e) => {
                tracing::warn!(user_id = %new_user.id, error = %e, "Failed to send verification email");
                email_sent = false;
            }
        }

        new_user.id
    };

    // Assign specified roles
    for role in payload.roles {
        state
            .keycloak_service
            .assign_realm_role_to_user(&admin_token, &user_id, &role)
            .await?;

        if role == "coop_admin" {
            // Required client roles for coop_admin
            let realm_management_roles = vec![
                "manage-users",
                "view-users",
                "query-users",
                "query-groups",
                "view-realm",
                "query-clients",
                "view-clients",
            ];
            let account_roles = vec!["view-groups"];

            for role in realm_management_roles {
                state
                    .keycloak_service
                    .assign_client_role_to_user(&admin_token, &user_id, "realm-management", role)
                    .await?;
            }

            for role in account_roles {
                state
                    .keycloak_service
                    .assign_client_role_to_user(&admin_token, &user_id, "account", role)
                    .await?;
            }
        } else if role == "coop_user" {
            // Required client roles for coop_user
            let realm_management_roles = vec!["query-groups", "view-users"];
            let account_roles = vec!["view-groups"];

            for role in realm_management_roles {
                state
                    .keycloak_service
                    .assign_client_role_to_user(&admin_token, &user_id, "realm-management", role)
                    .await?;
            }

            for role in account_roles {
                state
                    .keycloak_service
                    .assign_client_role_to_user(&admin_token, &user_id, "account", role)
                    .await?;
            }
        }
    }

    // Add user to the cooperation group
    state
        .keycloak_service
        .add_user_to_group(&admin_token, &user_id, &group_id)
        .await?;

    let response = AddMemberResponse {
        user_id: user_id.clone(),
        email: payload.email,
        email_sent,
        message: if email_sent {
            "User added successfully".to_string()
        } else {
            "User added but failed to send verification email".to_string()
        },
    };

    Ok((StatusCode::CREATED, Json(response)))
}

/// Get all members of a group
#[utoipa::path(
    get,
    path = "/admin/groups/{group_id}/members",
    tag = "User",
    params(("group_id" = String, Path, description = "Group ID")),
    responses((status = 200, description = "OK", body = Vec<KeycloakUser>))
)]
pub async fn get_group_members(
    State(state): State<AppState>,
    Extension(_token): Extension<String>,
    Path(group_id): Path<String>,
) -> AppResult<impl IntoResponse> {
    let admin_token = state.keycloak_service.get_admin_token().await?;
    let members = state
        .keycloak_service
        .get_group_members(&admin_token, &group_id)
        .await?;
    Ok(Json(members))
}

/// Delete a user by ID
#[utoipa::path(
    delete,
    path = "/admin/users/{user_id}",
    tag = "User",
    params(("user_id" = String, Path, description = "User ID")),
    responses((status = 204, description = "No Content"))
)]
pub async fn delete_user(
    State(state): State<AppState>,
    Extension(_token): Extension<String>,
    Path(user_id): Path<String>,
) -> AppResult<impl IntoResponse> {
    let admin_token = state.keycloak_service.get_admin_token().await?;
    state.keycloak_service.delete_user(&admin_token, &user_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Resend verification email to a group member
#[utoipa::path(
    post,
    path = "/admin/groups/{group_id}/members/{user_id}/resend-verification",
    tag = "User",
    params(
        ("group_id" = String, Path, description = "Cooperation Group ID"),
        ("user_id" = String, Path, description = "User ID")
    ),
    responses((status = 204, description = "No Content"))
)]
pub async fn resend_member_verification_email(
    State(state): State<AppState>,
    Extension(_token): Extension<String>,
    Path((_group_id, user_id)): Path<(String, String)>,
) -> AppResult<impl IntoResponse> {
    let admin_token = state.keycloak_service.get_admin_token().await?;
    state
        .keycloak_service
        .trigger_email_verification(&admin_token, &user_id, None)
        .await
        .map_err(|e| crate::error::AppError::InternalServerError(
            format!("Failed to send verification email: {}", e)
        ))?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(serde::Deserialize, utoipa::ToSchema)]
pub struct UpdateUserDimensionsRequest {
    pub dimension_ids: Vec<String>,
    pub email: Option<String>,
}

/// Update assigned dimensions for a user
#[utoipa::path(
    put,
    path = "/admin/users/{user_id}/dimensions",
    tag = "User",
    params(("user_id" = String, Path, description = "User ID")),
    request_body = UpdateUserDimensionsRequest,
    responses((status = 204, description = "No Content"))
)]
pub async fn update_user_dimensions(
    State(state): State<AppState>,
    Extension(_token): Extension<String>,
    Path(user_id): Path<String>,
    Json(request): Json<UpdateUserDimensionsRequest>,
) -> AppResult<impl IntoResponse> {
    let admin_token = state.keycloak_service.get_admin_token().await?;
    let attrs = serde_json::json!({ "assigned_dimensions": request.dimension_ids });
    state
        .keycloak_service
        .update_user_attributes(&admin_token, &user_id, attrs, request.email.as_deref())
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
