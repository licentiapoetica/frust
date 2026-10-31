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
    r#"CREATE TABLE IF NOT EXISTS upload_api_keys (
      id serial PRIMARY KEY,
      name text NOT NULL,
      key_hash text NOT NULL UNIQUE,
      max_bytes bigint DEFAULT NULL,
      revoked boolean DEFAULT false NOT NULL,
      created_at timestamp with time zone DEFAULT now() NOT NULL,
      last_used_at timestamp with time zone
    )"#,
    r#"CREATE TABLE IF NOT EXISTS chat_uploads (
      id bigserial PRIMARY KEY,
      slug text NOT NULL UNIQUE,
      key_id integer REFERENCES upload_api_keys(id) ON DELETE SET NULL,
      original_name text DEFAULT '' NOT NULL,
      mime text NOT NULL,
      mime_hint text DEFAULT '' NOT NULL,
      size_bytes bigint DEFAULT 0 NOT NULL,
      delete_token_hash text NOT NULL,
      created_at timestamp with time zone DEFAULT now() NOT NULL,
      expires_at timestamp with time zone NOT NULL
    )"#,
    r#"CREATE INDEX IF NOT EXISTS idx_chat_uploads_expires_at ON chat_uploads(expires_at)"#,
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
];
