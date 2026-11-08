-- Text size per frontend: {"<frontend>": percent} (100 = default size).
-- The old single user_options.font_size (one size for every frontend) becomes the default
-- frontend's (f0ckm) entry, then the column is dropped.
ALTER TABLE user_options ADD COLUMN IF NOT EXISTS font_sizes jsonb;
DO $$ BEGIN
    IF EXISTS (SELECT 1 FROM information_schema.columns WHERE table_name = 'user_options' AND column_name = 'font_size') THEN
        UPDATE user_options SET font_sizes = jsonb_build_object('f0ckm', font_size) || coalesce(font_sizes, '{}'::jsonb)
            WHERE font_size IS NOT NULL;
        ALTER TABLE user_options DROP COLUMN font_size;
    END IF;
END $$;
