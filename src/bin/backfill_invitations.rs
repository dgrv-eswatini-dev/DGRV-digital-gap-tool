use dgat_backend::{config, services::keycloak::KeycloakService};

/// One-time migration: copies the legacy `invited_organization` attribute onto
/// `org.ro.active` for existing users so they are tracked by the new pending
/// invitation logic.
///
/// Run: `cargo run --bin backfill_invitations`
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenv::dotenv().ok();
    let config = config::load_config()?;
    let keycloak_service = KeycloakService::new(config.clone());

    let token = keycloak_service.get_admin_token().await?;
    let migrated = keycloak_service
        .migrate_legacy_invitation_attribute(&token)
        .await?;

    println!("Migration complete: {} user(s) migrated.", migrated);
    Ok(())
}
