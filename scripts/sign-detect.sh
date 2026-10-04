#!/usr/bin/env bash
# Decide whether release signing is configured for a platform.
#
# Reads only the PRESENCE of credentials from the environment and writes
# `enabled=true|false` (and `provider=...` on Windows) to $GITHUB_OUTPUT.
# Nothing is signed unless every credential of a provider is present. A
# half-configured provider is an error, so a typo in a secret name can never
# silently produce an unsigned stable release.
set -euo pipefail

platform="${1:?usage: sign-detect.sh macos|windows}"
out="${GITHUB_OUTPUT:-/dev/stdout}"

present() { [[ -n "${!1:-}" ]]; }

# Prints complete, partial, or absent for the named environment variables.
state() {
  local set=0 total=0 name
  for name in "$@"; do
    total=$((total + 1))
    if present "$name"; then set=$((set + 1)); fi
  done
  if [[ $set -eq $total ]]; then
    echo complete
  elif [[ $set -eq 0 ]]; then
    echo absent
  else
    echo partial
  fi
}

emit() {
  echo "enabled=$1" >> "$out"
  echo "provider=$2" >> "$out"
}

disabled() {
  echo "Release signing is not enabled for $platform: $1"
  emit false none
  exit 0
}

fail() {
  echo "::error::Signing is partially configured for $platform: $1" >&2
  exit 1
}

# Signing is only ever attempted for a human-dispatched stable release. Pull
# request runs, including same-repository ones, never sign.
if [[ "${GITHUB_EVENT_NAME:-workflow_dispatch}" != workflow_dispatch ]]; then
  disabled "only manually dispatched stable releases are signed."
fi

case "$platform" in
  macos)
    cert_vars=(APPLE_CERTIFICATE APPLE_CERTIFICATE_PASSWORD)
    apple_id_vars=(APPLE_ID APPLE_TEAM_ID APPLE_APP_PASSWORD)
    api_key_vars=(APPLE_API_KEY_P8 APPLE_API_KEY_ID APPLE_API_ISSUER_ID)
    cert="$(state "${cert_vars[@]}")"
    apple_id="$(state "${apple_id_vars[@]}")"
    api_key="$(state "${api_key_vars[@]}")"

    if [[ $cert == absent && $apple_id == absent && $api_key == absent ]]; then
      disabled "no Apple signing secrets are configured; the package stays ad-hoc signed."
    fi
    [[ $cert == complete ]] ||
      fail "need all of: ${cert_vars[*]} (Developer ID certificate)."
    [[ $apple_id != partial ]] ||
      fail "APPLE_ID, APPLE_TEAM_ID, and APPLE_APP_PASSWORD must all be set together."
    [[ $api_key != partial ]] ||
      fail "APPLE_API_KEY_P8, APPLE_API_KEY_ID, and APPLE_API_ISSUER_ID must all be set together."
    [[ $apple_id == complete || $api_key == complete ]] ||
      fail "a signed app must also be notarized: set the Apple ID trio or the API key trio."
    if [[ $apple_id == complete && $api_key == complete ]]; then
      echo "Both notarization credential sets are present; the App Store Connect API key is used."
    fi
    echo "Developer ID signing and notarization are enabled for macOS."
    emit true apple
    ;;
  windows)
    azure_secrets=(AZURE_CLIENT_ID AZURE_TENANT_ID AZURE_SUBSCRIPTION_ID)
    azure_vars=(AZURE_SIGNING_ENDPOINT AZURE_SIGNING_ACCOUNT AZURE_SIGNING_PROFILE)
    signpath_vars=(SIGNPATH_API_TOKEN SIGNPATH_ORGANIZATION_ID SIGNPATH_PROJECT_SLUG SIGNPATH_SIGNING_POLICY_SLUG)
    azure="$(state "${azure_secrets[@]}" "${azure_vars[@]}")"
    signpath="$(state "${signpath_vars[@]}")"

    if [[ $azure == absent && $signpath == absent ]]; then
      disabled "no Azure Artifact Signing or SignPath configuration is present; packages stay unsigned."
    fi
    [[ $azure != partial ]] ||
      fail "Azure needs all of: ${azure_secrets[*]} (secrets) and ${azure_vars[*]} (variables)."
    [[ $signpath != partial ]] ||
      fail "SignPath needs all of: ${signpath_vars[*]} (SIGNPATH_API_TOKEN is a secret, the rest are variables)."
    if [[ $azure == complete ]]; then
      if [[ $signpath == complete ]]; then
        echo "Both providers are configured; Azure Artifact Signing (the default) is used."
      fi
      echo "Azure Artifact Signing is enabled for Windows."
      emit true azure
    else
      echo "SignPath is enabled for Windows."
      emit true signpath
    fi
    ;;
  *)
    echo "unknown platform: $platform" >&2
    exit 2
    ;;
esac
