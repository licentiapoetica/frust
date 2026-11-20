-- Registration with a passkey besides the password: an application keeps the passkey
-- (serialized webauthn::NewPasskey) until an admin approves it; the account gets it then.
ALTER TABLE account_applications ADD COLUMN IF NOT EXISTS passkey JSONB;
