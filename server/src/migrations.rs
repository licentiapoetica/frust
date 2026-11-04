//! Schema changes applied at every start (idempotent), from index.mjs.

pub const STARTUP_MIGRATIONS: &[&str] = &[
    r#"ALTER TABLE "user" ADD COLUMN IF NOT EXISTS groups text[] DEFAULT '{}'"#,
    r#"ALTER TABLE user_options ADD COLUMN IF NOT EXISTS banner_file character varying(255) DEFAULT NULL"#,
    r#"ALTER TABLE user_options ADD COLUMN IF NOT EXISTS banner_position character varying(50) DEFAULT 'center'"#,
    r#"ALTER TABLE user_options ADD COLUMN IF NOT EXISTS banner_size character varying(50) DEFAULT 'cover'"#,
    r#"ALTER TABLE user_options ADD COLUMN IF NOT EXISTS banner_repeat character varying(50) DEFAULT 'repeat'"#,
    r#"ALTER TABLE items ADD COLUMN IF NOT EXISTS expires_at bigint DEFAULT NULL"#,
    r#"ALTER TABLE items ADD COLUMN IF NOT EXISTS width integer DEFAULT NULL"#,
    r#"ALTER TABLE items ADD COLUMN IF NOT EXISTS height integer DEFAULT NULL"#,
    r#"ALTER TABLE items ADD COLUMN IF NOT EXISTS original_filename text DEFAULT NULL"#,
    r#"ALTER TABLE items ADD COLUMN IF NOT EXISTS title text DEFAULT NULL"#,
    r#"ALTER TABLE items ADD COLUMN IF NOT EXISTS uploader_ip character varying(128) DEFAULT NULL"#,
    r#"ALTER TABLE anon_activity_log ALTER COLUMN ip DROP NOT NULL"#,
    r#"ALTER TABLE items ADD COLUMN IF NOT EXISTS is_album boolean DEFAULT false"#,
    r#"ALTER TABLE items ADD COLUMN IF NOT EXISTS album_count integer DEFAULT 0"#,
    r#"CREATE TABLE IF NOT EXISTS album_items (
      id SERIAL PRIMARY KEY,
      item_id INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
      dest CHARACTER VARYING(60) NOT NULL,
      mime CHARACTER VARYING(100) NOT NULL,
      size INTEGER NOT NULL,
      checksum CHARACTER VARYING(255) NOT NULL,
      phash TEXT,
      width INTEGER,
      height INTEGER,
      order_index INTEGER NOT NULL DEFAULT 0,
      created_at TIMESTAMP WITH TIME ZONE DEFAULT NOW()
    )"#,
    r#"CREATE INDEX IF NOT EXISTS idx_album_items_item_id ON album_items(item_id, order_index ASC)"#,
    r#"ALTER TABLE album_items ADD COLUMN IF NOT EXISTS slug character varying(60) DEFAULT NULL"#,
    r#"CREATE INDEX IF NOT EXISTS idx_album_items_slug ON album_items(slug)"#,
    r#"CREATE TABLE IF NOT EXISTS album_items_tags_assign (
      album_item_id INTEGER NOT NULL REFERENCES album_items(id) ON DELETE CASCADE,
      tag_id INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
      user_id INTEGER REFERENCES "user"(id) ON DELETE SET DEFAULT,
      created_at TIMESTAMP WITH TIME ZONE DEFAULT NOW(),
      PRIMARY KEY (album_item_id, tag_id)
    )"#,
    r#"CREATE INDEX IF NOT EXISTS idx_album_items_tags_assign_tag_id ON album_items_tags_assign(tag_id)"#,
    r#"CREATE INDEX IF NOT EXISTS idx_album_items_tags_assign_album_item_id ON album_items_tags_assign(album_item_id)"#,
    // chat uploads for external clients were removed
    r#"DROP TABLE IF EXISTS chat_uploads"#,
    r#"DROP TABLE IF EXISTS upload_api_keys"#,
    // migrations/rust_webauthn.sql
    r#"CREATE TABLE IF NOT EXISTS webauthn_credentials (
      id serial PRIMARY KEY,
      user_id integer NOT NULL REFERENCES "user"(id) ON DELETE CASCADE,
      cred_id text NOT NULL UNIQUE,
      user_handle uuid NOT NULL,
      passkey jsonb NOT NULL,
      name text,
      aaguid text,
      created_at timestamp with time zone DEFAULT now() NOT NULL,
      last_used timestamp with time zone DEFAULT now()
    )"#,
    r#"CREATE INDEX IF NOT EXISTS idx_webauthn_credentials_user_id ON webauthn_credentials(user_id)"#,
    // routes/wordfilter.mjs created its table at startup
    r#"CREATE TABLE IF NOT EXISTS public.wordfilter (
      id SERIAL PRIMARY KEY,
      word VARCHAR(255) NOT NULL UNIQUE,
      replacement VARCHAR(255) NOT NULL,
      created_at TIMESTAMPTZ DEFAULT NOW()
    )"#,
    // URL metadata cache (routes/apiv2/index.mjs)
    r#"CREATE TABLE IF NOT EXISTS meta_cache (
      url TEXT PRIMARY KEY,
      data JSONB,
      created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
    )"#,
    // migrations/add_session_passkey_credential.sql: the passkey that opened a session
    r#"ALTER TABLE user_sessions ADD COLUMN IF NOT EXISTS passkey_credential_id TEXT"#,
    // migrations/add_report_categories.sql
    r#"ALTER TABLE reports ADD COLUMN IF NOT EXISTS categories text[] DEFAULT '{}'"#,
    // migrations/add_invite_requests.sql: anonymous users asking admins for an invite token
    r#"CREATE TABLE IF NOT EXISTS invite_requests (
      id SERIAL PRIMARY KEY,
      user_id INTEGER REFERENCES "user"(id) ON DELETE SET NULL,
      fingerprint VARCHAR(128) NOT NULL,
      ip_hash VARCHAR(128),
      reason TEXT DEFAULT '',
      status VARCHAR(16) DEFAULT 'pending' CHECK (status IN ('pending', 'approved', 'denied', 'revoked')),
      token_id INTEGER REFERENCES invite_tokens(id) ON DELETE SET NULL,
      reviewed_by INTEGER REFERENCES "user"(id) ON DELETE SET NULL,
      reviewed_at TIMESTAMP WITH TIME ZONE,
      created_at TIMESTAMP WITH TIME ZONE DEFAULT NOW()
    )"#,
    r#"CREATE INDEX IF NOT EXISTS idx_invite_requests_fingerprint ON invite_requests(fingerprint)"#,
    r#"CREATE INDEX IF NOT EXISTS idx_invite_requests_status ON invite_requests(status)"#,
    r#"CREATE INDEX IF NOT EXISTS idx_invite_requests_pending ON invite_requests(created_at) WHERE (status = 'pending')"#,
    // migrations/fix_null_visibility.sql, then make the column NOT NULL (only touches the table once)
    r#"DO $$ BEGIN
      IF EXISTS (SELECT 1 FROM information_schema.columns
                 WHERE table_schema = 'public' AND table_name = 'items' AND column_name = 'visibility' AND is_nullable = 'YES') THEN
        UPDATE items SET visibility = 0 WHERE visibility IS NULL;
        ALTER TABLE items ALTER COLUMN visibility SET NOT NULL;
      END IF;
    END $$"#,
];

#[cfg(test)]
mod tests {
    #[test]
    fn one_statement_each() {
        for m in super::STARTUP_MIGRATIONS {
            assert!(!m.contains("r#\"") && !m.contains("\"#"), "merged statements: {m}");
            assert!(m.matches("CREATE TABLE").count() <= 1, "more than one CREATE TABLE: {m}");
        }
    }
}
