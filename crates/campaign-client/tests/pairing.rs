#![allow(clippy::unwrap_used)] // tests panic on failure, which is the point

//! End-to-end tests of pairing a device with a short code (PAKE).

mod support;

use campaign_auth::{
    DeviceKey, PublicKey, Role,
    pairing::{JoinRequest, PairingCode},
    protocol::{Device, Principal},
};
use campaign_client::{
    ClientError,
    auth::AuthClient,
    pairing::{self, Invitation, PairError},
};
use support::{Auth, Party};

/// The DM's device hosting `invitation` in the background, approving (or
/// not) whoever turns up, and recording who that was.
fn host(
    auth: &Auth,
    party: &Party,
    invitation: &Invitation,
    approve: bool,
) -> tokio::task::JoinHandle<(Result<Device, PairError>, Option<JoinRequest>)> {
    host_vouching_for(auth, party, invitation, approve, auth.key)
}

fn host_vouching_for(
    auth: &Auth,
    party: &Party,
    invitation: &Invitation,
    approve: bool,
    server_key: PublicKey,
) -> tokio::task::JoinHandle<(Result<Device, PairError>, Option<JoinRequest>)> {
    let dm = auth.client().with_token(&party.dm.token);
    let key = DeviceKey::from_seed(party.dm_key.seed());
    let campaign = party.campaign.clone();
    let invitation = invitation.clone();
    tokio::spawn(async move {
        let mut asked = None;
        let result = pairing::host(&dm, &key, &campaign, server_key, &invitation, |request| {
            asked = Some(request.clone());
            approve
        })
        .await;
        (result, asked)
    })
}

async fn invite(auth: &Auth, party: &Party, role: Role) -> Invitation {
    auth.client()
        .with_token(&party.dm.token)
        .invite(role)
        .await
        .unwrap()
}

/// What the joiner types: the code as text, as read out by the DM.
fn spoken(invitation: &Invitation) -> PairingCode {
    invitation.code.to_string().parse().unwrap()
}

#[tokio::test]
async fn a_spoken_code_pairs_a_device() {
    let auth = Auth::start().await;
    let party = auth.campaign().await;
    let invitation = invite(&auth, &party, Role::Player).await;
    let hosting = host(&auth, &party, &invitation, true);

    let laptop = DeviceKey::generate();
    let joined = pairing::join(
        &auth.client(),
        &spoken(&invitation),
        &laptop,
        "Vex's laptop",
    )
    .await
    .unwrap();
    assert_eq!(joined.campaign, party.campaign);
    assert_eq!(joined.server_key, auth.key);
    assert_eq!(joined.cert.cert.issuer, party.dm_key.public());

    let (admitted, asked) = hosting.await.unwrap();
    let asked = asked.unwrap();
    assert_eq!(
        (asked.key, asked.name.as_str()),
        (laptop.public(), "Vex's laptop")
    );
    assert_eq!(admitted.unwrap().cert, joined.cert);

    let session = auth.login(party.id(), &laptop).await;
    assert_eq!(session.claims.role, Principal::Player);
    assert_eq!(session.claims.name, "Vex's laptop");
}

#[tokio::test]
async fn a_dm_can_pair_their_own_second_device() {
    let auth = Auth::start().await;
    let party = auth.campaign().await;
    let invitation = invite(&auth, &party, Role::Dm).await;
    let hosting = host(&auth, &party, &invitation, true);
    let phone = DeviceKey::generate();
    pairing::join(&auth.client(), &spoken(&invitation), &phone, "DM phone")
        .await
        .unwrap();
    hosting.await.unwrap().0.unwrap();
    assert_eq!(
        auth.login(party.id(), &phone).await.claims.role,
        Principal::Dm
    );
}

#[tokio::test]
async fn a_wrong_guess_burns_the_code() {
    let auth = Auth::start().await;
    let party = auth.campaign().await;
    let invitation = invite(&auth, &party, Role::Player).await;
    let hosting = host(&auth, &party, &invitation, true);

    // Right slot, wrong words: what an attacker who heard the number gets.
    let mut guess = PairingCode::generate(invitation.code.slot);
    while guess == invitation.code {
        guess = PairingCode::generate(invitation.code.slot);
    }
    let attacker = DeviceKey::generate();
    // The host cancels at once, so the joiner hears straight away rather
    // than waiting for the pairing to expire.
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        pairing::join(&auth.client(), &guess, &attacker, "Mallory"),
    )
    .await
    .expect("a wrong guess is reported promptly");
    assert!(matches!(result, Err(PairError::WrongCode)), "{result:?}");
    let (hosted, asked) = hosting.await.unwrap();
    assert!(matches!(hosted, Err(PairError::WrongCode)), "{hosted:?}");
    assert!(asked.is_none(), "the DM is never even asked");
    // The host cancelled the pairing: it's gone, not merely claimed.
    let gone = auth
        .client()
        .with_token(&party.dm.token)
        .cancel_pairing(invitation.code.slot)
        .await;
    assert!(matches!(gone, Err(ClientError::NotFound)), "{gone:?}");

    // The real player, with the right code, is too late: the code is gone.
    let player = DeviceKey::generate();
    let late = pairing::join(&auth.client(), &spoken(&invitation), &player, "Vex").await;
    assert!(matches!(late, Err(PairError::Unavailable)), "{late:?}");
    assert!(
        auth.client()
            .login(party.id(), &attacker, None)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn a_code_admits_only_one_device() {
    let auth = Auth::start().await;
    let party = auth.campaign().await;
    let invitation = invite(&auth, &party, Role::Player).await;
    let hosting = host(&auth, &party, &invitation, true);
    pairing::join(
        &auth.client(),
        &spoken(&invitation),
        &DeviceKey::generate(),
        "Vex",
    )
    .await
    .unwrap();
    hosting.await.unwrap().0.unwrap();

    let second = pairing::join(
        &auth.client(),
        &spoken(&invitation),
        &DeviceKey::generate(),
        "Vax",
    )
    .await;
    assert!(matches!(second, Err(PairError::Unavailable)), "{second:?}");
}

#[tokio::test]
async fn the_dm_can_decline() {
    let auth = Auth::start().await;
    let party = auth.campaign().await;
    let invitation = invite(&auth, &party, Role::Player).await;
    let hosting = host(&auth, &party, &invitation, false);
    let stranger = DeviceKey::generate();
    let result = pairing::join(&auth.client(), &spoken(&invitation), &stranger, "Who?").await;
    assert!(matches!(result, Err(PairError::Declined)), "{result:?}");
    assert!(matches!(hosting.await.unwrap().0, Err(PairError::Declined)));
    assert!(
        auth.client()
            .login(party.id(), &stranger, None)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn joiners_check_the_server_against_what_the_dm_knows() {
    let auth = Auth::start().await;
    let party = auth.campaign().await;
    let invitation = invite(&auth, &party, Role::Player).await;
    // The DM's device knows a different auth service than the one the
    // joiner reached (as if the joiner were pointed at an impostor).
    let elsewhere = DeviceKey::generate().public();
    let hosting = host_vouching_for(&auth, &party, &invitation, true, elsewhere);
    let result = pairing::join(
        &auth.client(),
        &spoken(&invitation),
        &DeviceKey::generate(),
        "Vex",
    )
    .await;
    assert!(matches!(result, Err(PairError::BadAnswer(_))), "{result:?}");
    let _ = hosting.await;
}

#[tokio::test]
async fn only_the_hosting_dm_device_controls_a_pairing() {
    let auth = Auth::start().await;
    let party = auth.campaign().await;
    let vex = party.player(&auth, "Vex").await;
    let as_player = auth
        .client()
        .with_token(&vex.token)
        .invite(Role::Player)
        .await;
    assert!(
        matches!(as_player, Err(ClientError::Forbidden(_))),
        "{as_player:?}"
    );

    let invitation = invite(&auth, &party, Role::Player).await;
    // Another campaign's DM can't cancel it (or speak for its host).
    let rival = auth.campaign().await;
    let cancelled = AuthClient::new(&auth.url)
        .with_token(&rival.dm.token)
        .cancel_pairing(invitation.code.slot)
        .await;
    assert!(
        matches!(cancelled, Err(ClientError::NotFound)),
        "{cancelled:?}"
    );
    // Its own host can.
    auth.client()
        .with_token(&party.dm.token)
        .cancel_pairing(invitation.code.slot)
        .await
        .unwrap();
    let late = pairing::join(
        &auth.client(),
        &spoken(&invitation),
        &DeviceKey::generate(),
        "Vex",
    )
    .await;
    assert!(matches!(late, Err(PairError::Unavailable)), "{late:?}");
}
