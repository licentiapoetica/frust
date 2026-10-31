-- Passkeys of the Rust server (webauthn-rs). Each row holds a serialized webauthn_rs::prelude::Passkey.
-- Replaces passkey_credentials (hand-rolled ES256 storage of the Node server), which stays untouched.
CREATE TABLE IF NOT EXISTS webauthn_credentials (
  id serial PRIMARY KEY,
  user_id integer NOT NULL REFERENCES "user"(id) ON DELETE CASCADE,
  cred_id text NOT NULL UNIQUE,
  user_handle uuid NOT NULL,
  passkey jsonb NOT NULL,
  name text,
  aaguid text,
  created_at timestamp with time zone DEFAULT now() NOT NULL,
  last_used timestamp with time zone DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_webauthn_credentials_user_id ON webauthn_credentials(user_id);
