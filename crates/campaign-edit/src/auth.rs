//! `campaign-edit auth ...`

use std::collections::HashMap;

use anyhow::{Context, Result, bail};
use campaign_auth::pairing::PairingCode;
use campaign_auth::{DeviceCert, PublicKey, Role, protocol::Device};
use campaign_client::{auth::AuthClient, pairing};

use crate::{
    cli::AuthCommand,
    identity::{Identity, Profile},
};

pub async fn run(command: AuthCommand, auth_server: &str) -> Result<()> {
    let identity = Identity::locate()?;
    match command {
        AuthCommand::Status => match identity.profile()? {
            None if !identity.profiles()?.is_empty() => println!(
                "No campaign chosen. Pick one with: campaign-edit auth use <CAMPAIGN>\n{}",
                campaign_list(&identity)?
            ),
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
                let others = identity.profiles()?.len().saturating_sub(1);
                if others > 0 {
                    println!("          and {others} more: campaign-edit auth campaigns");
                }
                println!("Rules     {}", claims.edition);
                println!("Device    {} ({:?})", claims.name, claims.role);
                println!("Key       {}", claims.key);
                println!("Server    {}", profile.auth_server);
            }
        },
        AuthCommand::Campaigns => match identity.profiles()?.as_slice() {
            [] => println!("This device isn't in any campaign yet."),
            _ => print!("{}", campaign_list(&identity)?),
        },
        AuthCommand::Use { campaign } => {
            let profile = identity.select(&campaign)?;
            println!(
                "Now using {} ({}).",
                profile.campaign.name, profile.campaign.id
            );
        }
        AuthCommand::Forget { campaign } => {
            let (profile, was_current) = identity.forget(&campaign)?;
            println!(
                "Forgot {} ({}) on this device.",
                profile.campaign.name, profile.campaign.id
            );
            if was_current && !identity.profiles()?.is_empty() {
                println!("Pick another with: campaign-edit auth use <CAMPAIGN>");
            }
        }
        AuthCommand::Invite { dm, yes } => {
            let (profile, session) = identity.login().await?;
            let host = AuthClient::new(&profile.auth_server).with_token(&session.token);
            let role = if dm { Role::Dm } else { Role::Player };
            let invitation = host.invite(role).await?;
            let code = &invitation.code;
            println!("Pairing code (works once, for 10 minutes):\n\n    {code}\n");
            println!("On the new device:");
            println!(
                "  campaign-edit auth pair {code} --name \"<device name>\" --auth-server {}\n",
                profile.auth_server
            );
            println!("Waiting for it to join… (Ctrl-C to give up)");
            let role_name = role_name(role);
            let device = pairing::host(
                &host,
                &identity.key()?,
                &profile.campaign,
                profile.server_key,
                &invitation,
                |request| {
                    yes || crate::editor::confirm(&format!(
                        "{} ({}) wants to join as {role_name}. Approve?",
                        request.name,
                        request.key.fingerprint()
                    ))
                    .unwrap_or(false)
                },
            )
            .await?;
            let cert = &device.cert.cert;
            println!(
                "{} ({}) has joined as {role_name}.",
                cert.name,
                cert.device.fingerprint()
            );
        }
        AuthCommand::Pair { code, name } => {
            let code: PairingCode = code.parse()?;
            let auth = AuthClient::new(auth_server);
            let key = identity.key()?;
            println!("Pairing… (your DM may need to approve)");
            let joined = pairing::join(&auth, &code, &key, &name).await?;
            identity.save_profile(&Profile {
                auth_server: auth_server.to_string(),
                server_key: joined.server_key,
                campaign: joined.campaign.clone(),
            })?;
            let (_, session) = identity.login().await?;
            println!(
                "Joined {} as {} ({}, key {}).",
                joined.campaign.name,
                session.claims.name,
                role_name(joined.cert.cert.role),
                key.public().fingerprint()
            );
            switched_hint(&identity)?;
        }
        AuthCommand::Key => {
            println!("{}", identity.key()?.public());
            eprintln!("\nGive this to your DM. Once they've approved it, run:");
            eprintln!("  campaign-edit auth join <CAMPAIGN ID>");
        }
        AuthCommand::Setup { code, name } => {
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
            println!("Add players with:  campaign-edit auth invite");
            switched_hint(&identity)?;
        }
        AuthCommand::Join { campaign } => {
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
            switched_hint(&identity)?;
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

/// After joining: the new campaign is now in use; say how to get back.
fn switched_hint(identity: &Identity) -> Result<()> {
    if identity.profiles()?.len() > 1 {
        println!("It's now the campaign in use; switch with: campaign-edit auth use <CAMPAIGN>");
    }
    Ok(())
}

/// The campaigns this device is in, the one in use marked with `*`.
fn campaign_list(identity: &Identity) -> Result<String> {
    let current = identity.profile()?.map(|p| p.campaign.id);
    let profiles = identity.profiles()?;
    let width = profiles
        .iter()
        .map(|p| p.campaign.name.chars().count())
        .max()
        .unwrap_or(0);
    Ok(profiles
        .iter()
        .map(|p| {
            let marker = if current == Some(p.campaign.id) {
                "*"
            } else {
                " "
            };
            format!(
                "{marker} {:<width$}  {}  {} rules  {}\n",
                p.campaign.name, p.campaign.id, p.campaign.edition, p.auth_server
            )
        })
        .collect())
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
        let role = role_name(cert.role);
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

fn role_name(role: Role) -> &'static str {
    match role {
        Role::Dm => "DM",
        Role::Player => "player",
    }
}
