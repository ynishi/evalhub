//! Append-only audit rows.
//!
//! `audit (id, at, actor_user_id, actor_token_id, ns, action, subject,
//! detail jsonb)`. Written in the same transaction as the action it
//! records, for: record creation, version append, tombstone, settings
//! change, label change, org membership change, token issue and revoke,
//! registry writes. Reads are `GET /audit?ns=&cursor=`, restricted to
//! namespaces the caller holds `admin` on.
//!
//! Rows are never updated or deleted by the application. The table has no
//! `UPDATE` or `DELETE` grant for the application role in the migration,
//! so this is enforced by Postgres, not by discipline.
