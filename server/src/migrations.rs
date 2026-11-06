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
    // migrations/add_private_unlisted_uploads.sql (slugs are filled in by backfill_slugs at boot)
    r#"ALTER TABLE items ADD COLUMN IF NOT EXISTS visibility smallint DEFAULT 0 NOT NULL"#,
    r#"ALTER TABLE items ADD COLUMN IF NOT EXISTS slug character varying(16) UNIQUE"#,
    r#"ALTER TABLE user_options ADD COLUMN IF NOT EXISTS default_upload_visibility smallint DEFAULT 0"#,
    // migrations/add_expiring_uploads.sql
    r#"CREATE INDEX IF NOT EXISTS idx_items_expires_at ON items(expires_at) WHERE ((expires_at IS NOT NULL) AND (is_purged = false))"#,
    // migrations/add_reporter_ip.sql, add_anon_activity_and_bans.sql
    r#"ALTER TABLE reports ADD COLUMN IF NOT EXISTS reporter_ip text"#,
    r#"ALTER TABLE comments ADD COLUMN IF NOT EXISTS ip character varying(128)"#,
    r#"ALTER TABLE meme_templates ADD COLUMN IF NOT EXISTS category character varying(100) DEFAULT 'General'"#,
    // anonymous accounts (migrations/add_anon_identities.sql, add_passkey_credentials.sql, add_hardware_fingerprint_bans.sql)
    r#"CREATE TABLE IF NOT EXISTS anon_identities (
      id SERIAL PRIMARY KEY,
      user_id integer NOT NULL REFERENCES "user"(id) ON DELETE CASCADE,
      pubkey text UNIQUE,
      fingerprint text UNIQUE,
      credential_id text UNIQUE,
      created_at timestamp with time zone DEFAULT now(),
      last_seen timestamp with time zone DEFAULT now(),
      created_ip character varying(128),
      last_ip character varying(128),
      hw_fingerprint character varying(128)
    )"#,
    r#"CREATE INDEX IF NOT EXISTS idx_anon_identities_user_id ON anon_identities(user_id)"#,
    r#"CREATE INDEX IF NOT EXISTS idx_anon_identities_hw ON anon_identities(hw_fingerprint)"#,
    // migrations/add_anon_activity_and_bans.sql
    r#"CREATE TABLE IF NOT EXISTS anon_activity_log (
      id SERIAL PRIMARY KEY,
      user_id integer NOT NULL,
      fingerprint character varying(128),
      hw_fingerprint character varying(128),
      action character varying(64) NOT NULL,
      target_id integer,
      ip character varying(128),
      user_agent text,
      details jsonb,
      created_at timestamp with time zone DEFAULT now()
    )"#,
    r#"CREATE INDEX IF NOT EXISTS idx_anon_act_user_id ON anon_activity_log(user_id)"#,
    r#"CREATE INDEX IF NOT EXISTS idx_anon_act_action ON anon_activity_log(action)"#,
    r#"CREATE INDEX IF NOT EXISTS idx_anon_act_ip ON anon_activity_log(ip)"#,
    r#"CREATE INDEX IF NOT EXISTS idx_anon_act_created ON anon_activity_log(created_at)"#,
    r#"CREATE INDEX IF NOT EXISTS idx_anon_activity_hw ON anon_activity_log(hw_fingerprint)"#,
    r#"CREATE TABLE IF NOT EXISTS banned_ips (
      id SERIAL PRIMARY KEY,
      ip character varying(128) NOT NULL UNIQUE,
      ip_hash character varying(128),
      reason text DEFAULT 'Banned by administrator',
      banned_by integer,
      created_at timestamp with time zone DEFAULT now(),
      expires_at timestamp with time zone
    )"#,
    r#"CREATE INDEX IF NOT EXISTS idx_banned_ips_hash ON banned_ips(ip_hash)"#,
    r#"CREATE TABLE IF NOT EXISTS banned_fingerprints (
      id SERIAL PRIMARY KEY,
      fingerprint character varying(128) NOT NULL UNIQUE,
      pubkey text,
      reason text DEFAULT 'Banned by administrator',
      banned_by integer,
      created_at timestamp with time zone DEFAULT now(),
      expires_at timestamp with time zone
    )"#,
    r#"CREATE TABLE IF NOT EXISTS banned_hardware_fingerprints (
      id SERIAL PRIMARY KEY,
      hw_fingerprint character varying(128) NOT NULL UNIQUE,
      banned_by integer REFERENCES "user"(id) ON DELETE SET NULL,
      reason text,
      created_at timestamp with time zone DEFAULT now() NOT NULL,
      expires_at timestamp with time zone
    )"#,
    r#"CREATE INDEX IF NOT EXISTS idx_banned_hw_expires ON banned_hardware_fingerprints(expires_at)"#,
    // migrations/add_user_recommendation_affinity.sql
    r#"CREATE TABLE IF NOT EXISTS user_tag_affinity (
      user_id integer NOT NULL REFERENCES "user"(id) ON DELETE CASCADE,
      tag_id integer NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
      score real DEFAULT 0.0,
      interaction_count integer DEFAULT 1,
      last_interacted timestamp with time zone DEFAULT now(),
      PRIMARY KEY (user_id, tag_id)
    )"#,
    r#"CREATE INDEX IF NOT EXISTS idx_user_tag_affinity_user_score ON user_tag_affinity(user_id, score DESC)"#,
    r#"CREATE TABLE IF NOT EXISTS user_creator_affinity (
      user_id integer NOT NULL REFERENCES "user"(id) ON DELETE CASCADE,
      creator_username text NOT NULL,
      score real DEFAULT 0.0,
      interaction_count integer DEFAULT 1,
      last_interacted timestamp with time zone DEFAULT now(),
      PRIMARY KEY (user_id, creator_username)
    )"#,
    r#"CREATE INDEX IF NOT EXISTS idx_user_creator_affinity_user_score ON user_creator_affinity(user_id, score DESC)"#,
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
    r#"CREATE INDEX IF NOT EXISTS idx_album_items_dest ON album_items(dest)"#,
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
    // the xD score was removed
    r#"DROP TRIGGER IF EXISTS trg_comments_xd_score ON comments"#,
    r#"DROP FUNCTION IF EXISTS trg_update_xd_score()"#,
    r#"DROP FUNCTION IF EXISTS update_item_xd_score(bigint)"#,
    r#"DROP INDEX IF EXISTS idx_items_xd_score"#,
    r#"ALTER TABLE items DROP COLUMN IF EXISTS xd_score"#,
    r#"ALTER TABLE user_options DROP COLUMN IF EXISTS min_xd_score"#,
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
    // older comment poll tables: votes keyed by (poll_id, user_id) without an id, missing constraints
    r#"DO $$ BEGIN
      IF NOT EXISTS (SELECT 1 FROM information_schema.columns
                     WHERE table_schema = 'public' AND table_name = 'comment_poll_votes' AND column_name = 'id') THEN
        ALTER TABLE comment_poll_votes DROP CONSTRAINT IF EXISTS comment_poll_votes_pkey;
        DROP INDEX IF EXISTS uq_cpv_poll_user;
        ALTER TABLE comment_poll_votes ADD COLUMN id SERIAL PRIMARY KEY;
        ALTER TABLE comment_poll_votes ADD CONSTRAINT comment_poll_votes_poll_id_user_id_key UNIQUE (poll_id, user_id);
      END IF;
      IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'comment_poll_votes_user_id_fkey') THEN
        DELETE FROM comment_poll_votes v WHERE NOT EXISTS (SELECT 1 FROM "user" u WHERE u.id = v.user_id);
        ALTER TABLE comment_poll_votes ADD CONSTRAINT comment_poll_votes_user_id_fkey FOREIGN KEY (user_id) REFERENCES "user"(id) ON DELETE CASCADE;
      END IF;
      IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'comment_polls_comment_id_key') THEN
        ALTER TABLE comment_polls ADD CONSTRAINT comment_polls_comment_id_key UNIQUE (comment_id);
      END IF;
      IF EXISTS (SELECT 1 FROM information_schema.columns
                 WHERE table_schema = 'public' AND table_name = 'comment_poll_options' AND column_name = 'sort_order' AND is_nullable = 'YES') THEN
        UPDATE comment_poll_options SET sort_order = 0 WHERE sort_order IS NULL;
        ALTER TABLE comment_poll_options ALTER COLUMN sort_order SET NOT NULL;
      END IF;
    END $$"#,
    r#"CREATE INDEX IF NOT EXISTS idx_comment_poll_options_poll ON comment_poll_options(poll_id)"#,
    r#"CREATE INDEX IF NOT EXISTS idx_comment_poll_votes_poll ON comment_poll_votes(poll_id)"#,
    r#"CREATE INDEX IF NOT EXISTS idx_comment_poll_votes_user ON comment_poll_votes(user_id)"#,
    // older defaults and column types
    r#"ALTER TABLE user_options ALTER COLUMN comment_display_mode SET DEFAULT 1"#,
    r#"ALTER TABLE user_options ALTER COLUMN username_color TYPE character varying"#,
    r#"ALTER TABLE dm_attachments ALTER COLUMN expires_at SET DEFAULT (now() + '90 days'::interval)"#,
    // no longer used: old views over items, tag aliases, the tags_nsfp table (NSFP tags are `nsfp` in
    // the config), the old Node passkey table (passkeys live in webauthn_credentials) and versioned TOS
    r#"DROP VIEW IF EXISTS items_li"#,
    r#"DROP VIEW IF EXISTS items_sfw"#,
    r#"DROP TABLE IF EXISTS tags_nsfp"#,
    r#"DROP TABLE IF EXISTS tags_alias"#,
    r#"DROP TABLE IF EXISTS passkey_credentials"#,
    r#"DROP TABLE IF EXISTS user_tos_first_seen"#,
    r#"DROP TABLE IF EXISTS user_tos_acceptance"#,
    r#"DROP TABLE IF EXISTS tos_versions"#,
    // plain indexes that repeat the index of a UNIQUE constraint (dropped only while that constraint exists)
    r#"DO $$ DECLARE p text[]; BEGIN
      FOREACH p SLICE 1 IN ARRAY ARRAY[
        ['idx_items_slug', 'items_slug_key'],
        ['idx_anon_identities_credential_id', 'anon_identities_credential_id_key'],
        ['idx_anon_identities_fingerprint', 'anon_identities_fingerprint_key'],
        ['idx_anon_identities_pubkey', 'anon_identities_pubkey_key'],
        ['idx_banned_fp_fp', 'banned_fingerprints_fingerprint_key'],
        ['idx_banned_hw_fp', 'banned_hardware_fingerprints_hw_fingerprint_key'],
        ['idx_banned_ips_ip', 'banned_ips_ip_key'],
        ['idx_comment_polls_comment_id', 'comment_polls_comment_id_key'],
        ['idx_link_token_token', 'link_token_token_key'],
        ['idx_user_api_keys_api_key', 'user_api_keys_api_key_key']
      ] LOOP
        IF EXISTS (SELECT 1 FROM pg_constraint WHERE conname = p[2] AND connamespace = 'public'::regnamespace) THEN
          EXECUTE format('DROP INDEX IF EXISTS public.%I', p[1]);
        END IF;
      END LOOP;
    END $$"#,
    r#"DROP INDEX IF EXISTS idx_user_halls_assign_item_id"#,
    r#"DROP INDEX IF EXISTS idx_user_halls_assign_user_id"#,
    // /b/<dest> lookups (media_gate)
    r#"CREATE INDEX IF NOT EXISTS idx_items_dest ON items(dest)"#,
    // Website font weight (100-900, NULL: the font's own) and text size (percent, NULL: 100)
    r#"ALTER TABLE user_options ADD COLUMN IF NOT EXISTS font_weight smallint"#,
    r#"ALTER TABLE user_options ADD COLUMN IF NOT EXISTS font_size smallint"#,
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
