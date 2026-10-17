//! `campaign-edit auth ...`

use std::collections::HashMap;

use anyhow::{Context, Result, bail};
use campaign_auth::pairing::PairingCode;
use campaign_auth::{
    DeviceCert, PublicKey, Role,
    protocol::{Device, Session},
};
use campaign_client::{auth::AuthClient, pairing};
use uuid::Uuid;

use crate::{
    cli::{AuthCommand, OwnersCommand},
    identity::{Identity, Profile},
};

pub async fn run(command: AuthCommand, auth_server: &str) -> Result<()> {
    let identity = Identity::locate()?;
    match command {
        AuthCommand::Status => status(&identity).await,
        AuthCommand::Campaigns => {
            match identity.profiles()?.as_slice() {
                [] => println!("This device isn't in any campaign yet."),
                _ => print!("{}", campaign_list(&identity)?),
            }
            Ok(())
        }
        AuthCommand::Use { campaign } => {
            let profile = identity.select(&campaign)?;
            println!(
                "Now using {} ({}).",
                profile.campaign.name, profile.campaign.id
            );
            Ok(())
        }
        AuthCommand::Forget { campaign } => forget(&identity, &campaign),
        AuthCommand::Invite { dm, yes } => invite(&identity, auth_server, dm, yes).await,
        AuthCommand::Pair { code, name } => pair(&identity, auth_server, &code, &name).await,
        AuthCommand::Key => {
            println!("{}", identity.key()?.public());
            eprintln!("\nGive this to your DM. Once they've approved it, run:");
            eprintln!("  campaign-edit auth join <CAMPAIGN ID>");
            Ok(())
        }
        AuthCommand::Setup { code, name } => setup(&identity, auth_server, &code, &name).await,
        AuthCommand::Join { campaign } => join(&identity, auth_server, campaign).await,
        AuthCommand::Approve {
            public_key,
            name,
            dm,
        } => approve(&identity, public_key, &name, dm).await,
        AuthCommand::Devices => {
            let (_, session, auth) = logged_in(&identity).await?;
            print!(
                "{}",
                device_table(&auth.devices().await?, session.claims.key)
            );
            Ok(())
        }
        AuthCommand::Revoke { device } => revoke(&identity, &device).await,
    }
}

/// Logged in to the campaign in use, with a client for its auth server.
async fn logged_in(identity: &Identity) -> Result<(Profile, Session, AuthClient)> {
    let (profile, session) = identity.login().await?;
    let auth = AuthClient::new(&profile.auth_server).with_token(&session.token);
    Ok((profile, session, auth))
}

async fn status(identity: &Identity) -> Result<()> {
    match identity.profile()? {
        None if !identity.profiles()?.is_empty() => println!(
            "No campaign chosen. Pick one with: campaign-edit auth use <CAMPAIGN>\n{}",
            campaign_list(identity)?
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
    }
    Ok(())
}

fn forget(identity: &Identity, campaign: &str) -> Result<()> {
    let (profile, was_current) = identity.forget(campaign)?;
    println!(
        "Forgot {} ({}) on this device.",
        profile.campaign.name, profile.campaign.id
    );
    if was_current && !identity.profiles()?.is_empty() {
        println!("Pick another with: campaign-edit auth use <CAMPAIGN>");
    }
    Ok(())
}

/// Shows a pairing code and waits for a device to join with it, asking
/// before letting it in (unless `yes`). The new device is told to pair
/// through `auth_server` if given (such as the HTTPS proxy's address,
/// when this device reaches the services directly), else the address this
/// device uses.
async fn invite(identity: &Identity, auth_server: &str, dm: bool, yes: bool) -> Result<()> {
    let (profile, _, host) = logged_in(identity).await?;
    let address = if auth_server == crate::cli::AUTH_SERVER {
        &profile.auth_server
    } else {
        auth_server
    };
    let role = if dm { Role::Dm } else { Role::Player };
    let invitation = host.invite(role).await?;
    let code = &invitation.code;
    println!("Pairing code (works once, for 10 minutes):\n\n    {code}\n");
    println!("On the new device:");
    println!("  campaign-edit auth pair {code} --name \"<device name>\" --auth-server {address}\n");
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
    Ok(())
}

/// Joins a campaign with a code from `invite`.
async fn pair(identity: &Identity, auth_server: &str, code: &str, name: &str) -> Result<()> {
    let code: PairingCode = code.parse()?;
    let auth = AuthClient::new(auth_server);
    let key = identity.key()?;
    println!("Pairing… (your DM may need to approve)");
    let joined = pairing::join(&auth, &code, &key, name).await?;
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
    switched_hint(identity)
}

/// Becomes DM of a new campaign, with its one-time setup code.
async fn setup(identity: &Identity, auth_server: &str, code: &str, name: &str) -> Result<()> {
    let auth = AuthClient::new(auth_server);
    let server_key = auth.server_key().await?;
    let campaign = auth.setup(code, &identity.key()?, name).await?;
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
    switched_hint(identity)
}

/// Joins a campaign whose DM has approved this device's key.
async fn join(identity: &Identity, auth_server: &str, campaign: Uuid) -> Result<()> {
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
    switched_hint(identity)
}

/// Lets another device's key into the campaign (the manual alternative to
/// `invite`).
async fn approve(identity: &Identity, public_key: PublicKey, name: &str, dm: bool) -> Result<()> {
    let (profile, _, auth) = logged_in(identity).await?;
    let role = if dm { Role::Dm } else { Role::Player };
    let cert = DeviceCert::issue(
        &identity.key()?,
        profile.campaign.id,
        public_key,
        role,
        name,
    )?;
    auth.add_device(&cert).await?;
    println!("Approved {name} ({}).", public_key.fingerprint());
    println!("They can now run:");
    println!(
        "  campaign-edit auth join {} --auth-server {}",
        profile.campaign.id, profile.auth_server
    );
    Ok(())
}

async fn revoke(identity: &Identity, device: &str) -> Result<()> {
    let (_, _, auth) = logged_in(identity).await?;
    let devices = auth.devices().await?;
    let target = find_device(&devices, device)?;
    let revoked = auth.revoke(target.device).await?;
    println!(
        "Revoked {} ({}).",
        revoked.cert.cert.name,
        target.device.fingerprint()
    );
    Ok(())
}

/// Shows, or changes, which devices play a character.
pub async fn owners(
    characters: &campaign_client::Client,
    character: Uuid,
    change: Option<OwnersCommand>,
) -> Result<()> {
    let (_, _, auth) = logged_in(&Identity::locate()?).await?;
    let devices = auth.devices().await?;
    let mut current = characters.get(character).await?;
    if let Some(change) = change {
        let mut owners = current.owners.clone();
        match change {
            OwnersCommand::Add { device } => {
                let cert = find_device(&devices, &device)?;
                if cert.role != Role::Player {
                    bail!(
                        "{} is a DM device, which plays every character already",
                        cert.name
                    );
                }
                let key = cert.device.to_string();
                if !owners.contains(&key) {
                    owners.push(key);
                }
            }
            OwnersCommand::Remove { device } => {
                let key = find_device(&devices, &device)?.device.to_string();
                owners.retain(|owner| *owner != key);
            }
        }
        current = characters.set_owners(character, &owners).await?;
    }
    let name = &current.sheet.name;
    if current.owners.is_empty() {
        println!("Nobody plays {name} yet; only DM devices change them.");
        println!("Let a player's device play them with: campaign-edit {name} owners add <DEVICE>");
        return Ok(());
    }
    println!("{name} is played by:");
    for owner in &current.owners {
        let named = devices
            .iter()
            .find(|d| d.cert.cert.device.to_string() == *owner)
            .map_or("(a device no longer in the campaign)", |d| {
                d.cert.cert.name.as_str()
            });
        println!("  {}  {named}", &owner[..8.min(owner.len())]);
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
