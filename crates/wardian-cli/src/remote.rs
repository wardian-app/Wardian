//! Remote pairing is app-owned. The CLI forwards the exact origin and payload.

use crate::args::{RemoteArgs, RemoteCommand};
use crate::errors::CliError;

/// Forwards an operator request to the running app and renders its opaque JSON.
pub fn handle(args: &RemoteArgs) -> Result<String, CliError> {
    handle_with(args, crate::live::remote_client_pairing_offer)
}

fn handle_with(
    args: &RemoteArgs,
    create_offer: impl FnOnce(&str) -> std::io::Result<serde_json::Value>,
) -> Result<String, CliError> {
    match &args.command {
        RemoteCommand::ClientPairingOffer { client_origin } => {
            // Origin syntax, canonicalization, and allowlist policy belong to the app.
            let payload = create_offer(client_origin).map_err(crate::control_error)?;
            crate::render_json(payload)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::{Cli, Command};
    use clap::Parser;
    use serde_json::json;

    fn args() -> RemoteArgs {
        let cli = Cli::try_parse_from([
            "wardian",
            "remote",
            "client-pairing-offer",
            "--client-origin",
            "https://hud.example:9443",
        ])
        .unwrap();
        let Command::Remote(args) = cli.command else {
            panic!("expected remote command")
        };
        args
    }

    #[test]
    fn remote_offer_requires_explicit_origin() {
        assert!(Cli::try_parse_from(["wardian", "remote", "client-pairing-offer"]).is_err());
    }

    #[test]
    fn remote_offer_forwards_exact_origin_once_and_preserves_opaque_payload() {
        let payload = json!({"nonce":"opaque-secret", "future_field":{"nested":[1,2]}});
        let output = handle_with(&args(), |origin| {
            assert_eq!(origin, "https://hud.example:9443");
            Ok(payload.clone())
        })
        .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&output).unwrap(),
            payload
        );
        assert!(output.ends_with('\n'));
    }

    #[test]
    fn remote_offer_preserves_backend_rejection() {
        let error = handle_with(&args(), |_| {
            Err(std::io::Error::other(
                crate::live::ControlEndpointError::new(
                    "client_origin_not_allowlisted",
                    "origin rejected",
                ),
            ))
        })
        .unwrap_err();
        assert_eq!(error.code, "client_origin_not_allowlisted");
        assert_eq!(error.message, "origin rejected");
    }

    #[test]
    fn remote_offer_reports_unavailable_app_without_offline_fallback() {
        let error = handle_with(&args(), |_| {
            Err(std::io::Error::from(std::io::ErrorKind::ConnectionRefused))
        })
        .unwrap_err();
        assert_eq!(error.code, "app_not_running");
    }
}
