-- Text size per frontend: {"<frontend>": percent} (100 = default size).
-- user_options.font_size stays the fallback for frontends without an entry.
ALTER TABLE user_options ADD COLUMN IF NOT EXISTS font_sizes jsonb;
