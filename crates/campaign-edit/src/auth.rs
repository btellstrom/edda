//! `campaign-edit auth ...`

use std::collections::HashMap;

use anyhow::{Context, Result, bail};
use campaign_auth::{DeviceCert, PublicKey, Role, protocol::Device};
use campaign_client::auth::AuthClient;

use crate::{
    cli::AuthCommand,
    identity::{Identity, Profile},
};

pub async fn run(command: AuthCommand, auth_server: &str) -> Result<()> {
    let identity = Identity::locate()?;
    match command {
        AuthCommand::Status => match identity.profile()? {
            None => println!(
                "Not in a campaign. This device's key: {}",
                identity.key()?.public()
            ),
            Some(_) => {
                let (profile, session) = identity.login().await?;
                let claims = session.claims;
                println!(
                    "Campaign  {} ({})",
                    profile.campaign.name, profile.campaign.id
                );
                println!("Device    {} ({:?})", claims.name, claims.role);
                println!("Key       {}", claims.key);
                println!("Server    {}", profile.auth_server);
            }
        },
        AuthCommand::Key => {
            println!("{}", identity.key()?.public());
            eprintln!("\nGive this to your DM. Once they've approved it, run:");
            eprintln!("  campaign-edit auth join <CAMPAIGN ID>");
        }
        AuthCommand::Setup { code, name } => {
            refuse_second_campaign(&identity)?;
            let auth = AuthClient::new(auth_server);
            let server_key = auth.server_key().await?;
            let campaign = auth.setup(&code, &identity.key()?, &name).await?;
            identity.save_profile(&Profile {
                auth_server: auth_server.to_string(),
                server_key,
                campaign: campaign.clone(),
            })?;
            println!(
                "This device is now DM of {} ({}).",
                campaign.name, campaign.id
            );
            println!("Players join with:  campaign-edit auth key   (and send you the key)");
        }
        AuthCommand::Join { campaign } => {
            refuse_second_campaign(&identity)?;
            let auth = AuthClient::new(auth_server);
            let server_key = auth.server_key().await?;
            let session = auth
                .login(campaign, &identity.key()?, Some(server_key))
                .await
                .context("has the DM approved this device's key yet?")?;
            let campaign = auth.with_token(&session.token).campaign().await?;
            identity.save_profile(&Profile {
                auth_server: auth_server.to_string(),
                server_key,
                campaign: campaign.clone(),
            })?;
            println!("Joined {} as {}.", campaign.name, session.claims.name);
        }
        AuthCommand::Approve {
            public_key,
            name,
            dm,
        } => {
            let (profile, session) = identity.login().await?;
            let role = if dm { Role::Dm } else { Role::Player };
            let cert = DeviceCert::issue(
                &identity.key()?,
                profile.campaign.id,
                public_key,
                role,
                &name,
            )?;
            AuthClient::new(&profile.auth_server)
                .with_token(&session.token)
                .add_device(&cert)
                .await?;
            println!("Approved {name} ({}).", public_key.fingerprint());
            println!("They can now run:");
            println!(
                "  campaign-edit auth join {} --auth-server {}",
                profile.campaign.id, profile.auth_server
            );
        }
        AuthCommand::Devices => {
            let (profile, session) = identity.login().await?;
            let devices = AuthClient::new(&profile.auth_server)
                .with_token(&session.token)
                .devices()
                .await?;
            print!("{}", device_table(&devices, session.claims.key));
        }
        AuthCommand::Revoke { device } => {
            let (profile, session) = identity.login().await?;
            let auth = AuthClient::new(&profile.auth_server).with_token(&session.token);
            let devices = auth.devices().await?;
            let target = find_device(&devices, &device)?;
            let revoked = auth.revoke(target.device).await?;
            println!(
                "Revoked {} ({}).",
                revoked.cert.cert.name,
                target.device.fingerprint()
            );
        }
    }
    Ok(())
}

fn refuse_second_campaign(identity: &Identity) -> Result<()> {
    if let Some(profile) = identity.profile()? {
        bail!(
            "this device is already in {} ({}); use another CAMPAIGN_CONFIG_DIR for a second campaign",
            profile.campaign.name,
            profile.campaign.id
        );
    }
    Ok(())
}

fn find_device<'a>(devices: &'a [Device], query: &str) -> Result<&'a DeviceCert> {
    let active = devices
        .iter()
        .filter(|d| d.revoked_at.is_none())
        .map(|d| &d.cert.cert);
    let matches: Vec<_> = active
        .filter(|c| c.name.eq_ignore_ascii_case(query) || c.device.to_string().starts_with(query))
        .collect();
    match matches.as_slice() {
        [one] => Ok(one),
        [] => bail!("no active device named or keyed {query:?}"),
        _ => bail!("{query:?} matches several devices; use more of the key"),
    }
}

/// Every device with its certificate checked here, on this machine: the
/// signature must be valid and the issuer a DM device of the campaign (or,
/// for a campaign's first DM, the device itself).
fn device_table(devices: &[Device], me: PublicKey) -> String {
    let dms: HashMap<PublicKey, &str> = devices
        .iter()
        .filter(|d| d.cert.cert.role == Role::Dm)
        .map(|d| (d.cert.cert.device, d.cert.cert.name.as_str()))
        .collect();
    let width = devices
        .iter()
        .map(|d| d.cert.cert.name.len())
        .max()
        .unwrap_or(0);
    let mut out = String::new();
    for device in devices {
        let cert = &device.cert.cert;
        let vouched = match (device.cert.verify(), dms.get(&cert.issuer)) {
            (Err(_), _) => "INVALID SIGNATURE".to_string(),
            (Ok(_), _) if device.cert.is_self_signed() => "set up with a code".to_string(),
            (Ok(_), Some(issuer)) => format!("approved by {issuer}"),
            (Ok(_), None) => "UNKNOWN ISSUER".to_string(),
        };
        let role = match cert.role {
            Role::Dm => "DM",
            Role::Player => "player",
        };
        let status = match device.revoked_at {
            Some(at) => format!("  revoked {}", at.format("%Y-%m-%d")),
            None => String::new(),
        };
        let marker = if cert.device == me { "*" } else { " " };
        out += &format!(
            "{marker} {}  {:<width$}  {:<6}  {vouched}{status}\n",
            cert.device.fingerprint(),
            cert.name,
            role
        );
    }
    out
}
