#!/bin/bash

# ==== CONFIGURATION ====
export KEYCLOAK_BIN_DIR=/opt/keycloak/bin
export KEYCLOAK_SERVER=${KEYCLOAK_SERVER:-http://keycloak:8080/keycloak}
export TRUSTSTORE=/opt/keycloak/bin/truststore.jks
export REALM=digital-gap
export ADMIN_USER=admin
export ADMIN_PASS=admin123
export TRUSTSTORE_PASS=changeit
export NEW_USER_EMAIL=360@dgrv.coop
export TEMP_PASSWORD=dgrv@coop360
export NEW_USER_FIRSTNAME=fernando
export NEW_USER_LASTNAME=espinosa

echo "[a.sh] Current working directory: $(pwd)"
echo "[a.sh] Changing directory to ${KEYCLOAK_BIN_DIR}"
cd "${KEYCLOAK_BIN_DIR}"
echo "[a.sh] Now in: $(pwd)"

# Run-once guard to avoid re-provisioning
# Run-once guard is now handled more granularly below.

# Certificate and truststore logic is no longer needed for internal HTTP communication.

# --- 1. LOG IN AS ADMIN ON MASTER REALM (retry until ready) ---
login_ok=false
for i in {1..120}; do
  if ./kcadm.sh config credentials --server "${KEYCLOAK_SERVER}" --realm master --user "${ADMIN_USER}" --password "${ADMIN_PASS}"; then
    login_ok=true
    break
  fi
  echo "Waiting for Keycloak to start... ($i/120)"
  sleep 2
done

if [ "$login_ok" != true ]; then
  echo "Failed to authenticate with Keycloak after multiple attempts"
  exit 1
fi

# --- 2. CREATE THE USER (only if missing) ---
# Existence is checked against Keycloak itself, not a marker file: the marker lives in the
# container filesystem and is lost on every container recreate, which used to reset the
# admin password to the temporary one on each redeploy.
USER_EXISTS=$(./kcadm.sh get users -r "${REALM}" -q username="${NEW_USER_EMAIL}" -q exact=true \
    --fields id --server "${KEYCLOAK_SERVER}" 2>/dev/null | grep -c '"id"' || true)

if [ "${USER_EXISTS}" = "0" ]; then
    echo "[a.sh] User ${NEW_USER_EMAIL} not found, creating it with a temporary password..."
    if ./kcadm.sh create users \
        -r "${REALM}" \
        -s username="${NEW_USER_EMAIL}" \
        -s email="${NEW_USER_EMAIL}" \
        -s enabled=true \
        -s firstName="${NEW_USER_FIRSTNAME}" \
        -s lastName="${NEW_USER_LASTNAME}" \
        --server "${KEYCLOAK_SERVER}"; then
        # --- 3. SET TEMPORARY PASSWORD (new user only) ---
        ./kcadm.sh set-password \
            -r "${REALM}" \
            --username "${NEW_USER_EMAIL}" \
            --new-password "${TEMP_PASSWORD}" \
            --temporary \
            --server "${KEYCLOAK_SERVER}"
    else
        echo "[a.sh] Warning: could not create user ${NEW_USER_EMAIL}; password left untouched."
    fi
else
    echo "[a.sh] User ${NEW_USER_EMAIL} already exists; skipping creation and password reset."
fi

RUN_ONCE_MARKER="${KEYCLOAK_BIN_DIR}/.user_provisioned"


# --- 4. ASSIGN REALM-MANAGEMENT ROLES (replace or add/remove as needed) ---
./kcadm.sh add-roles -r "${REALM}" --uusername "${NEW_USER_EMAIL}" --cclientid realm-management \
  --rolename create-client \
  --rolename create-organization \
  --rolename impersonation \
  --rolename manage-authorization \
  --rolename manage-clients \
  --rolename manage-events \
  --rolename manage-identity-providers \
  --rolename manage-organizations \
  --rolename manage-realm \
  --rolename manage-users \
  --rolename publish-events \
  --rolename query-clients \
  --rolename query-groups \
  --rolename query-realms \
  --rolename query-users \
  --rolename realm-admin \
  --rolename view-authorization \
  --rolename view-clients \
  --rolename view-events \
  --rolename view-identity-providers \
  --rolename view-organizations \
  --rolename view-realm \
  --rolename view-users \
  --server "${KEYCLOAK_SERVER}"

# --- 4.1 ASSIGN ROLES TO THE BACKEND SERVICE ACCOUNT ---
echo "[a.sh] Assigning realm-admin roles to dgat-admin-client service account..."
# Use the internal service account username format
if ! ./kcadm.sh add-roles -r "${REALM}" \
  --uusername "service-account-dgat-admin-client" \
  --cclientid realm-management \
  --rolename realm-admin \
  --server "${KEYCLOAK_SERVER}"; then
  
  echo "[a.sh] Failed by username, trying to assign by Service Account User ID..."
  
  # Get the internal ID of the client first
  CLIENT_INTERNAL_ID=$(./kcadm.sh get clients -r "${REALM}" -q clientId=dgat-admin-client --fields id 2>/dev/null | grep -o '"id" : "[^"]*"' | head -1 | sed 's/"id" : "\(.*\)"/\1/')
  
  if [ -n "$CLIENT_INTERNAL_ID" ]; then
    # Get the service account user ID for that client
    SA_ID=$(./kcadm.sh get clients/$CLIENT_INTERNAL_ID/service-account-user -r "${REALM}" --fields id 2>/dev/null | grep -o '"id" : "[^"]*"' | head -1 | sed 's/"id" : "\(.*\)"/\1/')
    
    if [ -n "$SA_ID" ]; then
      ./kcadm.sh add-roles -r "${REALM}" --uid "$SA_ID" --cclientid realm-management --rolename realm-admin --server "${KEYCLOAK_SERVER}"
      echo "[a.sh] Successfully assigned roles to service account ID: $SA_ID"
    else
      echo "[a.sh] Error: Could not find service account user for client $CLIENT_INTERNAL_ID"
    fi
  else
    echo "[a.sh] Error: Could not find client dgat-admin-client"
  fi
fi

# --- 5. ASSIGN application_admin and drgv_admin realm roles ---
./kcadm.sh add-roles -r "${REALM}" --uusername "${NEW_USER_EMAIL}" --rolename application_admin --rolename dgrv_admin \
  --server "${KEYCLOAK_SERVER}"

echo ${KC_KC_SPI_EMAIL_DEFAULT_PASSWORD}

echo "[a.sh] Configuring realm email settings..."
./kcadm.sh update realms/"${REALM}" \
  -s 'smtpServer.host='"${KC_SPI_EMAIL_DEFAULT_HOST:-smtp.gmail.com}" \
  -s 'smtpServer.port='"${KC_SPI_EMAIL_DEFAULT_PORT:-25}" \
  -s 'smtpServer.from='"${KC_SPI_EMAIL_DEFAULT_FROM:-dgrvesw@gmail.comp}" \
  -s 'smtpServer.fromDisplayName='"${KC_SPI_EMAIL_DEFAULT_FROM_DISPLAY_NAME:-DGRV COOPERATION}" \
  -s 'smtpServer.user='"${KC_SPI_EMAIL_DEFAULT_USER:-dgrvesw@gmail.com}" \
  -s 'smtpServer.password='"${KC_SPI_EMAIL_DEFAULT_PASSWORD}" \
  -s 'smtpServer.ssl='"${KC_SPI_EMAIL_DEFAULT_SSL:-false}" \
  -s 'smtpServer.starttls='"${KC_SPI_EMAIL_DEFAULT_STARTTLS:-true}" \
  -s 'smtpServer.auth='"${KC_SPI_EMAIL_DEFAULT_AUTH:-true}" \
  -s 'smtpServer.replyTo='"${KC_SPI_EMAIL_DEFAULT_FROM:-noreply@dgrv.coop}" \
  -s 'smtpServer.replyToDisplayName='"${KC_SPI_EMAIL_DEFAULT_FROM_DISPLAY_NAME:-DGRV COOPERATION}" \
  -s 'smtpServer.envelopeFrom=' \
  -s 'smtpServer.debug=false' \
  --server "${KEYCLOAK_SERVER}"

echo "[a.sh] Email configuration completed successfully"

# --- Set realm frontend URL so action token links use the public HTTPS URL ---
KEYCLOAK_PUBLIC_URL="${KC_HOSTNAME_URL:-https://gat.dgrvcoop360.com/keycloak}"
echo "[a.sh] Setting realm frontendUrl to ${KEYCLOAK_PUBLIC_URL}..."
./kcadm.sh update realms/"${REALM}" \
  -s "attributes.frontendUrl=${KEYCLOAK_PUBLIC_URL}" \
  --server "${KEYCLOAK_SERVER}"
echo "[a.sh] Realm frontendUrl set successfully"

# --- Reset dgat-admin-client secret (realm export masks it with ***) ---
echo "[a.sh] Resetting dgat-admin-client secret..."
./kcadm.sh update clients/e2e2e2e2-e2e2-4e2e-b2e2-e2e2e2e2e2e2 -r "${REALM}" \
  -s secret="${DGAT_KEYCLOAK_CLIENT_SECRET:-dev-secret}" \
  --server "${KEYCLOAK_SERVER}"
echo "[a.sh] dgat-admin-client secret reset done."

# --- Add organization scope to dgat-client default scopes ---
echo "[a.sh] Adding organization scope to dgat-client default scopes..."
DGAT_CLIENT_ID=$(./kcadm.sh get clients -r "${REALM}" -q clientId=dgat-client --fields id 2>/dev/null | grep -o '"id" : "[^"]*"' | head -1 | sed 's/"id" : "\(.*\)"/\1/')
if [ -n "$DGAT_CLIENT_ID" ]; then
  ./kcadm.sh update clients/"${DGAT_CLIENT_ID}" -r "${REALM}" \
    -s 'defaultClientScopes=["web-origins","acr","profile","roles","basic","email","organization","user_attributes"]' \
    --server "${KEYCLOAK_SERVER}"
  echo "[a.sh] Organization scope added to dgat-client ID: ${DGAT_CLIENT_ID}"
else
  echo "[a.sh] Warning: Could not find dgat-client ID, trying hardcoded ID..."
  ./kcadm.sh update clients/644ba92b-94a9-4341-b1e7-69dad77dc594 -r "${REALM}" \
    -s 'defaultClientScopes=["web-origins","acr","profile","roles","basic","email","organization","user_attributes"]' \
    --server "${KEYCLOAK_SERVER}"
  echo "[a.sh] Organization scope applied via hardcoded client ID"
fi

# Mark provisioning as done to avoid re-running on subsequent starts
touch "${RUN_ONCE_MARKER}"
echo "[a.sh] Provisioning completed; marker created at ${RUN_ONCE_MARKER}"