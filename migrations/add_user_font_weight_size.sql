-- Website font weight (100-900, NULL: the font's own) and text size (percent, NULL: 100).
-- Also in f0ckm_schema.sql and server/src/migrations.rs (STARTUP_MIGRATIONS).
ALTER TABLE user_options ADD COLUMN IF NOT EXISTS font_weight smallint;
ALTER TABLE user_options ADD COLUMN IF NOT EXISTS font_size smallint;
