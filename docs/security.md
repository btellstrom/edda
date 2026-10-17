# Security

How devices join a campaign and log in, and what the services check.
What each role may do is in [Using edda](using-edda.md#who-may-do-what);
the services enforce all of it, whatever a client sends.

## Device keys

Every device (the DM's laptop, a player's phone) has its own Ed25519 key,
generated on the device and never sent anywhere: `edda` keeps it in
`~/.config/edda/device.key` (or `$EDDA_CONFIG_DIR`). Nobody has
passwords.

## Certificates

A device belongs to a campaign when a DM device of that campaign has
signed a certificate for it: *this key, this campaign, this role, this
name, signed by that DM key*. The auth service only accepts a certificate
signed by the DM device presenting it, and keeps it on record; `edda auth
devices` checks every certificate itself.

A campaign's first DM device is set up with a one-time code printed by
`auth-service new-campaign`, and signs its own certificate. If every DM
device is lost, whoever runs the server can print a new code with
`auth-service setup-code <CAMPAIGN>`.

**Roles.** DM devices admit and revoke devices; the campaign always keeps
at least one. Players use the services. Services (registered with
`auth-service add-service`) may log in to any campaign, so the combat
service can write hit points back.

Revoking a device stops it logging in; a token it already has stays valid
until it expires (at most 15 minutes), and an open WebSocket stays open.

## Pairing

Rather than copying keys around, the DM's device shows a code like
`7-tavern-dragon` that is read out and typed on the new device. The number
only picks the pairing on the auth service; the two words (from the EFF
short wordlist, 1,296 words) are a password that never leaves the two
devices. They run SPAKE2 with it (a PAKE: password-authenticated key
exchange), the auth service relaying the messages, and agree on a strong
key only if both used the same words. Nobody watching, the auth service
included, learns anything to guess the words against offline. Someone who
heard only the number gets one online guess: a wrong guess fails to
decrypt, the DM's device cancels the pairing, and only one device can ever
claim a code anyway. That makes two words (one chance in 1.7 million)
plenty.

Over the resulting channel (ChaCha20-Poly1305, a key per direction, from
HKDF-SHA256) the new device sends its key and name; the DM is asked to
approve, and the DM's device adds the device with a certificate and sends
back the certificate, the campaign, and the auth service's key. So a
paired device trusts the auth service because the DM's device vouched for
it, not merely because it answered first. Codes expire after 10 minutes.
The relay keeps messages in Postgres and clients long-poll, so the two
devices may reach different auth-service instances.

## Logging in

Logging in is challenge–response. The device asks for a challenge (a
random nonce, single-use, valid for a minute) and signs it together with
the campaign and the auth service's own key, so the signature is useless
anywhere else. In return it gets a token valid for 15 minutes: a PASETO
v4.public token, Ed25519-signed by the auth service, saying which device it
is, its campaign, role and name. The other services check tokens with the
auth service's public key alone, without calling it. Devices remember the
auth service's key from the first time they use it and refuse to log in to
any other.

**Signatures** always cover `"edda/v1/<purpose>\0"` followed by the
value's canonical binary (postcard) encoding, so a signature made for one
purpose (say, logging in) can never pass as another (a certificate).

## Running it safely

- Let players in only through the HTTPS proxy ([Hosting](hosting.md#letting-players-in-https)),
  never by publishing the services' own ports.
- Keep the [limits](hosting.md#limits) on wherever others can reach the
  servers, and `TRUST_PROXY=1` only where nothing but the proxy can reach
  the services.
- Back up ([Hosting](hosting.md#backups)), and keep the backups
  elsewhere: they hold every campaign and the combat service's key.
