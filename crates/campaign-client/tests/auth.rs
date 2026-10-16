//! End-to-end tests of the auth service: setting up campaigns, admitting
//! and revoking devices, and logging in. Mostly about what must *not* work.

mod support;

use campaign_auth::{
    DeviceCert, DeviceKey, Purpose, Role,
    protocol::{Challenge, ChallengeRequest, LoginProof, LoginRequest, Principal},
};
use campaign_client::ClientError;
use support::Auth;

#[tokio::test]
async fn setup_codes_work_once_and_make_a_dm() {
    let auth = Auth::start().await;
    let campaign = auth
        .store
        .create_campaign("Vox Machina", Default::default())
        .await
        .unwrap();
    let code = auth
        .store
        .new_setup_code(campaign.id, chrono::Duration::hours(1))
        .await
        .unwrap();

    let dm = DeviceKey::generate();
    let joined = auth.client().setup(&code, &dm, "DM laptop").await.unwrap();
    assert_eq!(joined, campaign);
    let session = auth.login(campaign.id, &dm).await;
    assert_eq!(session.claims.role, Principal::Dm);
    assert_eq!(session.claims.name, "DM laptop");
    assert_eq!(session.claims.key, dm.public());

    let thief = DeviceKey::generate();
    let again = auth.client().setup(&code, &thief, "Not the DM").await;
    assert!(
        matches!(again, Err(ClientError::Unauthorized(_))),
        "{again:?}"
    );
    let made_up = auth.client().setup("made-up", &thief, "Not the DM").await;
    assert!(
        matches!(made_up, Err(ClientError::Unauthorized(_))),
        "{made_up:?}"
    );
}

#[tokio::test]
async fn only_known_devices_can_log_in_and_only_with_their_key() {
    let auth = Auth::start().await;
    let party = auth.campaign().await;
    let stranger = DeviceKey::generate();
    let refused = auth.client().login(party.id(), &stranger, None).await;
    assert!(
        matches!(refused, Err(ClientError::Unauthorized(_))),
        "{refused:?}"
    );

    // Someone who knows the DM's public key (it's public) but not the
    // private key gets a challenge, but can't answer it.
    let http = reqwest::Client::new();
    let challenge: Challenge = http
        .post(format!("{}/login/challenge", auth.url))
        .json(&ChallengeRequest {
            campaign: party.id(),
            key: party.dm_key.public(),
        })
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let forged = LoginRequest {
        challenge: challenge.id,
        signature: stranger
            .sign(Purpose::Login, &LoginProof::from(&challenge))
            .unwrap(),
    };
    let response = http
        .post(format!("{}/login", auth.url))
        .json(&forged)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 401);
}

#[tokio::test]
async fn challenges_can_be_answered_only_once() {
    let auth = Auth::start().await;
    let party = auth.campaign().await;
    let http = reqwest::Client::new();
    let challenge: Challenge = http
        .post(format!("{}/login/challenge", auth.url))
        .json(&ChallengeRequest {
            campaign: party.id(),
            key: party.dm_key.public(),
        })
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(challenge.server, auth.key);
    let answer = LoginRequest {
        challenge: challenge.id,
        signature: party
            .dm_key
            .sign(Purpose::Login, &LoginProof::from(&challenge))
            .unwrap(),
    };
    let login = |answer| http.post(format!("{}/login", auth.url)).json(answer).send();
    assert_eq!(login(&answer).await.unwrap().status(), 200);
    assert_eq!(
        login(&answer).await.unwrap().status(),
        401,
        "a replayed answer is refused"
    );
}

#[tokio::test]
async fn devices_refuse_to_log_in_to_an_unexpected_server() {
    let auth = Auth::start().await;
    let party = auth.campaign().await;
    let impostor = DeviceKey::generate().public();
    let refused = auth
        .client()
        .login(party.id(), &party.dm_key, Some(impostor))
        .await;
    assert!(
        matches!(refused, Err(ClientError::Unauthorized(_))),
        "{refused:?}"
    );
}

#[tokio::test]
async fn only_dms_admit_devices_and_only_with_their_own_signature() {
    let auth = Auth::start().await;
    let party = auth.campaign().await;
    let vex = party.player(&auth, "Vex's laptop").await;
    assert_eq!(vex.claims.role, Principal::Player);
    assert_eq!(vex.claims.name, "Vex's laptop");

    let newcomer = DeviceKey::generate();
    let as_dm = auth.client().with_token(&party.dm.token);

    // A player can't admit anyone, even with a DM-signed certificate.
    let cert = DeviceCert::issue(
        &party.dm_key,
        party.id(),
        newcomer.public(),
        Role::Player,
        "New",
    )
    .unwrap();
    let by_player = auth.client().with_token(&vex.token).add_device(&cert).await;
    assert!(
        matches!(by_player, Err(ClientError::Forbidden(_))),
        "{by_player:?}"
    );

    // A certificate must be signed by the DM device presenting it...
    let other_dm = DeviceKey::generate();
    let cert =
        DeviceCert::issue(&other_dm, party.id(), newcomer.public(), Role::Dm, "New").unwrap();
    assert!(matches!(
        as_dm.add_device(&cert).await,
        Err(ClientError::Rejected(_))
    ));
    // ...be for this campaign...
    let cert = DeviceCert::issue(
        &party.dm_key,
        uuid::Uuid::new_v4(),
        newcomer.public(),
        Role::Player,
        "New",
    )
    .unwrap();
    assert!(matches!(
        as_dm.add_device(&cert).await,
        Err(ClientError::Rejected(_))
    ));
    // ...and say exactly what was signed.
    let mut cert = DeviceCert::issue(
        &party.dm_key,
        party.id(),
        newcomer.public(),
        Role::Player,
        "New",
    )
    .unwrap();
    cert.cert.role = Role::Dm;
    assert!(matches!(
        as_dm.add_device(&cert).await,
        Err(ClientError::Unauthorized(_))
    ));

    // Every device on record carries a certificate anyone can check.
    let devices = auth
        .client()
        .with_token(&vex.token)
        .devices()
        .await
        .unwrap();
    assert_eq!(devices.len(), 2);
    for device in &devices {
        let cert = device.cert.verify().unwrap();
        assert!(device.cert.is_self_signed() || cert.issuer == party.dm_key.public());
    }
}

#[tokio::test]
async fn revoked_devices_are_locked_out_but_the_last_dm_stays() {
    let auth = Auth::start().await;
    let party = auth.campaign().await;
    let as_dm = auth.client().with_token(&party.dm.token);
    let (vex_key, vex) = party.join(&auth, "Vex", Role::Player).await;

    let by_player = auth
        .client()
        .with_token(&vex.token)
        .revoke(party.dm_key.public())
        .await;
    assert!(matches!(by_player, Err(ClientError::Forbidden(_))));

    let revoked = as_dm.revoke(vex_key.public()).await.unwrap();
    assert!(revoked.revoked_at.is_some());
    let locked_out = auth.client().login(party.id(), &vex_key, None).await;
    assert!(matches!(locked_out, Err(ClientError::Unauthorized(_))));

    // The only DM device can't be revoked...
    let last = as_dm.revoke(party.dm_key.public()).await;
    assert!(matches!(last, Err(ClientError::Rejected(_))), "{last:?}");
    // ...but once there's a second, either can go.
    let (phone_key, phone) = party.join(&auth, "DM phone", Role::Dm).await;
    assert_eq!(phone.claims.role, Principal::Dm);
    auth.client()
        .with_token(&phone.token)
        .revoke(party.dm_key.public())
        .await
        .unwrap();
    assert!(
        auth.client()
            .login(party.id(), &party.dm_key, None)
            .await
            .is_err()
    );
    auth.login(party.id(), &phone_key).await;
}

#[tokio::test]
async fn an_admin_can_recover_a_campaign_with_a_new_setup_code() {
    let auth = Auth::start().await;
    let party = auth.campaign().await;
    // Every DM device is lost; the server's admin prints a new code.
    let code = auth
        .store
        .new_setup_code(party.id(), chrono::Duration::hours(1))
        .await
        .unwrap();
    let new_dm = DeviceKey::generate();
    auth.client()
        .setup(&code, &new_dm, "New DM laptop")
        .await
        .unwrap();
    let session = auth.login(party.id(), &new_dm).await;
    assert_eq!(session.claims.role, Principal::Dm);
}

#[tokio::test]
async fn services_can_log_in_to_any_campaign() {
    let auth = Auth::start().await;
    let (a, b) = (auth.campaign().await, auth.campaign().await);
    let service = DeviceKey::generate();
    assert!(auth.client().login(a.id(), &service, None).await.is_err());
    auth.store
        .add_service("combat-service", service.public())
        .await
        .unwrap();
    for party in [&a, &b] {
        let session = auth.login(party.id(), &service).await;
        assert_eq!(session.claims.role, Principal::Service);
        assert_eq!(session.claims.campaign, party.id());
    }
    let nowhere = auth
        .client()
        .login(uuid::Uuid::new_v4(), &service, None)
        .await;
    assert!(nowhere.is_err(), "but only to campaigns that exist");
}

#[tokio::test]
async fn deleting_a_campaign_locks_out_its_devices_and_leaves_others_alone() {
    let auth = Auth::start().await;
    let doomed = auth.campaign().await;
    let (vex, _) = doomed.join(&auth, "Vex's laptop", Role::Player).await;
    let other = auth.campaign().await;

    let (deleted, devices) = auth
        .store
        .delete_campaign(doomed.id())
        .await
        .unwrap()
        .expect("it exists");
    assert_eq!(deleted, doomed.campaign);
    assert_eq!(devices, 2);
    for key in [&doomed.dm_key, &vex] {
        let login = auth.client().login(doomed.id(), key, Some(auth.key)).await;
        assert!(
            matches!(login, Err(ClientError::Unauthorized(_))),
            "{login:?}"
        );
    }
    auth.login(other.id(), &other.dm_key).await;
    assert!(
        auth.store
            .delete_campaign(doomed.id())
            .await
            .unwrap()
            .is_none()
    );
}
