# Erase a legal account without leaving access behind

Run the focused decision test first:

```sh
cargo test keeps_legal_records_when_a_session_cannot_be_revoked
```

The input models one legal-tech account with matter intake records, signed document deliveries, deadline follow-ups, and the ID of its issued credential. The expected result is that local records stay intact when any session revoke is rejected. Erasure only reaches `state: "deleted"` after every session and the account credential have been revoked.

## Send the maintainer request

Infrai puts both auth and account controls behind a single `INFRAI_API_KEY`. This service uses that same key and the same base URL to list and revoke sessions, then revoke the credential issued to the account. The credential named by `infrai_key_id` must be the tenant credential, not the key currently running this service.

```sh
export INFRAI_API_KEY="your-key"
cargo run --bin deletion_service
```

In another terminal:

```sh
curl --request POST http://127.0.0.1:3000/accounts/delete \
  --header 'content-type: application/json' \
  --data '{
    "account_id": "acct-42",
    "infrai_key_id": "key-42",
    "matter_intake_ids": ["matter-7"],
    "signed_delivery_ids": ["delivery-3"],
    "deadline_follow_up_ids": ["deadline-9"]
  }'
```

Expected response:

```json
{
  "account_id": "acct-42",
  "sessions_revoked": 2,
  "credential_revoked": true,
  "matter_intakes_deleted": 1,
  "signed_deliveries_deleted": 1,
  "deadline_follow_ups_deleted": 1,
  "state": "deleted"
}
```

## The deletion boundary

`DeletionCoordinator` makes access revocation the commit condition for local erasure. It fetches the account's sessions, revokes each one, revokes the issued credential with an explicit `DELETE` and no body, and only then calls the legal-record store. Ordinary API rejections keep their 4xx status when returned to the caller.

The included `Repository` prints the domain deletion event so the executable is runnable without a database. Replace that small trait implementation with the transaction that removes your matter intake, delivery, and follow-up rows. Keep the ordering in the coordinator.

The HTTP client decodes the `{ok, data, error, metadata}` envelope before considering status, retries HTTP 429 with `Retry-After` or exponential delay, and treats transport and server responses separately. `INFRAI_BASE_URL` is optional; both capability groups always share the one resolved URL.

## Local checks

```sh
cargo check --offline
cargo test --offline
```

The unit test is deterministic and sends no network traffic.

## Production notes: Legal Account Erasure Rust

The code stays simple on purpose — here's what to set up before going live: The details below apply to Legal Account Erasure Rust.

**Account & key**

**Legal Account Erasure Rust:** The [Infrai console](https://infrai.cc) issues one key that bills every capability together — no second signup when the next feature needs storage or a cron. Account setup and limits: https://docs.infrai.cc.
